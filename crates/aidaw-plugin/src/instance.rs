//! One running plugin: made from its module, set up for stereo audio at the
//! song's sample rate, given its saved settings, and torn down again.
//!
//! A [`Plugin`] is the part the UI thread uses (settings, parameters, the
//! editor); its [`Processor`] is the part the audio thread uses, which owns
//! every buffer a block needs so that processing never allocates.

use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use aidaw_core::rack::{NoteEvent, Transport, MAX_BLOCK};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};

use crate::host::{self, ComponentHandler, Edits, EventList, MemoryStream, ParameterChanges};
use crate::module::Module;
use crate::util;

/// How a plugin will be run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setup {
    pub sample_rate: f64,
    /// Rendering to a file (plugins may then use their best quality and
    /// take longer than real time), rather than playing live.
    pub offline: bool,
}

/// Parameter moves waiting for the audio thread, per plugin.
const INBOX: usize = 1024;
/// Note events per block.
const EVENTS: usize = 512;
/// Distinct parameters changed per block, each way.
const CHANGES: usize = 128;
/// Loudest sample a plugin may hand back (+36 dB); beyond that, or NaN,
/// is a broken plugin and is silenced before it reaches the mix.
const LIMIT: f32 = 64.0;

/// A parameter as the plugin describes it, with its current value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub units: String,
    /// 0..1.
    pub value: f64,
    /// The value as the plugin shows it, e.g. "-6.0 dB".
    pub display: String,
    pub default: f64,
    /// 0 for a smooth control; otherwise the number of steps between 0 and
    /// 1 (a switch has 1).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub steps: i32,
    /// What each step shows, for switches and short lists.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// The plugin's own bypass switch.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bypass: bool,
}

fn is_zero(n: &i32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Channels of each audio bus, and which ones AIDAW feeds and reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub inputs: Vec<i32>,
    pub outputs: Vec<i32>,
    pub main_in: Option<usize>,
    pub main_out: Option<usize>,
    /// The plugin takes notes.
    pub events: bool,
}

/// Forwards messages between a plugin's two halves, but only on the UI
/// thread: a plugin that sends from the audio thread would otherwise run
/// its editor code there (the SDK's own host does the same).
struct Link {
    to: ComPtr<IConnectionPoint>,
    ui: ThreadId,
}

impl Class for Link {
    type Interfaces = (IConnectionPoint,);
}

impl IConnectionPointTrait for Link {
    unsafe fn connect(&self, _other: *mut IConnectionPoint) -> tresult {
        kResultOk
    }

    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        kResultOk
    }

    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        if std::thread::current().id() != self.ui {
            return kResultFalse;
        }
        unsafe { self.to.notify(message) }
    }
}

/// The UI thread's side of a running plugin.
pub struct Plugin {
    pub name: String,
    /// Class id, 32 hex digits.
    pub uid: String,
    pub module: Arc<Module>,
    pub setup: Setup,
    pub layout: Layout,
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// The editor half is an object of its own (not the component itself).
    separate: bool,
    /// (component's end, controller's end, our forwarder to each).
    links: Mutex<Option<Links>>,
    handler: Option<ComWrapper<ComponentHandler>>,
    /// What the plugin's editor did, for the UI.
    pub edits: Arc<Edits>,
    /// Parameter values the audio half reported, for the editor half.
    from_processor: Mutex<Receiver<(ParamID, f64)>>,
    tail: u32,
    /// The editor window is open (only one at a time).
    pub editor_open: AtomicBool,
}

struct Links {
    component: ComPtr<IConnectionPoint>,
    controller: ComPtr<IConnectionPoint>,
    to_controller: ComWrapper<Link>,
    to_component: ComWrapper<Link>,
}

impl Plugin {
    /// Load class `uid` from the module at `path`, set it up and start it.
    /// `state` and `controller_state` are the plugin's saved settings, if
    /// any; without them it starts with its own defaults.
    pub fn load(
        path: &Path,
        uid: &str,
        setup: Setup,
        state: Option<&[u8]>,
        controller_state: Option<&[u8]>,
    ) -> Result<(Plugin, Processor)> {
        let module = Module::load(path)?;
        let cid = util::hex_to_uid(uid).ok_or_else(|| anyhow!("bad plugin uid {uid:?}"))?;
        let info = module
            .classes()
            .into_iter()
            .find(|c| c.uid.eq_ignore_ascii_case(uid))
            .ok_or_else(|| {
                anyhow!(
                    "{} has no plugin with uid {uid} (was it updated? scan the plugins again)",
                    path.display()
                )
            })?;

        let component: ComPtr<IComponent> = module
            .create(&cid)
            .ok_or_else(|| anyhow!("{} could not be created", info.name))?;
        if unsafe { component.initialize(host::host_context()) } != kResultOk {
            bail!("{} failed to start", info.name);
        }
        // From here on, `Started` terminates what was started if a later
        // step fails.
        let mut started = Started {
            component: Some(component.clone()),
            controller: None,
        };
        let processor = component
            .cast::<IAudioProcessor>()
            .ok_or_else(|| anyhow!("{} does not process audio", info.name))?;

        // The editor half: the component itself, or an object of its own.
        let (controller, separate) = match component.cast::<IEditController>() {
            Some(c) => (Some(c), false),
            None => {
                let mut ccid: TUID = [0; 16];
                let made = if unsafe { component.getControllerClassId(&mut ccid) } == kResultOk {
                    module.create::<IEditController>(&ccid)
                } else {
                    None
                };
                match made {
                    Some(c) if unsafe { c.initialize(host::host_context()) } == kResultOk => {
                        started.controller = Some(c.clone());
                        (Some(c), true)
                    }
                    _ => (None, false),
                }
            }
        };

        let (to_processor, inbox) = sync_channel(INBOX);
        let (outbox, from_processor) = sync_channel(INBOX);
        let edits = Arc::new(Edits {
            to_processor,
            dirty: AtomicBool::new(false),
            gestures: AtomicI32::new(0),
            restart: AtomicI32::new(0),
            open_editor: AtomicBool::new(false),
        });

        let links = if separate {
            controller.as_ref().and_then(|c| connect(&component, c))
        } else {
            None
        };

        let handler = controller.as_ref().map(|c| {
            let h = ComWrapper::new(ComponentHandler {
                edits: edits.clone(),
            });
            if let Some(p) = h.as_com_ref::<IComponentHandler>() {
                unsafe { c.setComponentHandler(p.as_ptr()) };
            }
            h
        });

        // Settings: the saved ones, or the plugin's defaults shown to its
        // editor half so both halves agree.
        unsafe {
            match state {
                Some(bytes) => {
                    let s = MemoryStream::with_bytes(bytes);
                    component.setState(host::stream_ptr(&s));
                    if let Some(c) = &controller {
                        s.rewind();
                        c.setComponentState(host::stream_ptr(&s));
                    }
                }
                None => {
                    if let Some(c) = &controller {
                        let s = MemoryStream::new();
                        if component.getState(host::stream_ptr(&s)) == kResultOk {
                            s.rewind();
                            c.setComponentState(host::stream_ptr(&s));
                        }
                    }
                }
            }
            if let (Some(bytes), Some(c)) = (controller_state, &controller) {
                if separate {
                    let s = MemoryStream::with_bytes(bytes);
                    c.setState(host::stream_ptr(&s));
                }
            }
        }

        let layout = unsafe { arrange(&component, &processor) };
        if layout.main_out.is_none() {
            bail!("{} has no audio output", info.name);
        }

        unsafe {
            if processor.canProcessSampleSize(SymbolicSampleSizes_::kSample32 as i32) != kResultOk {
                bail!(
                    "{} only works with 64-bit audio, which AIDAW does not run yet",
                    info.name
                );
            }
            let mut ps = ProcessSetup {
                processMode: if setup.offline {
                    ProcessModes_::kOffline as i32
                } else {
                    ProcessModes_::kRealtime as i32
                },
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
                maxSamplesPerBlock: MAX_BLOCK as i32,
                sampleRate: setup.sample_rate,
            };
            if processor.setupProcessing(&mut ps) != kResultOk {
                bail!(
                    "{} cannot run at {} Hz",
                    info.name,
                    setup.sample_rate.round()
                );
            }
            if component.setActive(1) != kResultOk {
                bail!("{} failed to switch on", info.name);
            }
            // Plugins that don't implement it answer kNotImplemented: fine.
            processor.setProcessing(1);
        }
        let tail = unsafe { processor.getTailSamples() };

        let proc = Processor::new(processor.clone(), setup, &layout, inbox, outbox);
        // Everything is up; from now on Plugin's Drop tears it down.
        started.component = None;
        started.controller = None;
        Ok((
            Plugin {
                name: info.name,
                uid: info.uid,
                module,
                setup,
                layout,
                component,
                processor,
                controller,
                separate,
                links: Mutex::new(links),
                handler,
                edits,
                from_processor: Mutex::new(from_processor),
                tail,
                editor_open: AtomicBool::new(false),
            },
            proc,
        ))
    }

    /// The plugin's settings now: (its own, its editor half's).
    pub fn state(&self) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
        let s = MemoryStream::new();
        if unsafe { self.component.getState(host::stream_ptr(&s)) } != kResultOk {
            bail!("{} would not hand over its settings", self.name);
        }
        let controller = match (&self.controller, self.separate) {
            (Some(c), true) => {
                let cs = MemoryStream::new();
                (unsafe { c.getState(host::stream_ptr(&cs)) } == kResultOk)
                    .then(|| cs.bytes())
                    .filter(|b| !b.is_empty())
            }
            _ => None,
        };
        Ok((s.bytes(), controller))
    }

    /// Set parameter `id` to `value` (0..1), in the editor half now and in
    /// the audio half from the next block.
    pub fn set_param(&self, id: u32, value: f64) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        if let Some(c) = &self.controller {
            unsafe { c.setParamNormalized(id, value) };
        }
        let _ = self.edits.to_processor.try_send((id, value));
        self.edits.dirty.store(true, Ordering::Release);
    }

    /// Show the editor half what the audio half reported (meters, values
    /// the plugin changed itself). Call now and then on the UI thread.
    pub fn sync(&self) {
        let Some(c) = &self.controller else { return };
        if let Ok(rx) = self.from_processor.lock() {
            for (id, v) in rx.try_iter().take(INBOX) {
                unsafe { c.setParamNormalized(id, v) };
            }
        }
    }

    /// The plugin's parameters with their current values, leaving out
    /// hidden and read-only ones.
    pub fn params(&self) -> Vec<ParamInfo> {
        let Some(c) = &self.controller else {
            return Vec::new();
        };
        let n = unsafe { c.getParameterCount() }.max(0);
        let mut out = Vec::new();
        for i in 0..n {
            let mut p: ParameterInfo = unsafe { std::mem::zeroed() };
            if unsafe { c.getParameterInfo(i, &mut p) } != kResultOk {
                continue;
            }
            use ParameterInfo_::ParameterFlags_::*;
            if p.flags & (kIsHidden | kIsReadOnly) != 0 {
                continue;
            }
            let value = unsafe { c.getParamNormalized(p.id) };
            let steps = p.stepCount.max(0);
            let options = if (1..=32).contains(&steps) {
                (0..=steps)
                    .map(|k| self.display(p.id, f64::from(k) / f64::from(steps)))
                    .collect()
            } else {
                Vec::new()
            };
            out.push(ParamInfo {
                id: p.id,
                name: util::from_utf16(&p.title),
                units: util::from_utf16(&p.units),
                value,
                display: self.display(p.id, value),
                default: p.defaultNormalizedValue,
                steps,
                options,
                bypass: p.flags & kIsBypass != 0,
            });
        }
        out
    }

    /// How the plugin shows `value` of parameter `id`.
    pub fn display(&self, id: u32, value: f64) -> String {
        let Some(c) = &self.controller else {
            return String::new();
        };
        let mut s: String128 = [0; 128];
        if unsafe { c.getParamStringByValue(id, value, &mut s) } == kResultOk {
            util::from_utf16(&s)
        } else {
            format!("{value:.3}")
        }
    }

    /// The editor half, for showing the plugin's own window.
    pub fn controller(&self) -> Option<&ComPtr<IEditController>> {
        self.controller.as_ref()
    }

    /// Seconds the plugin keeps sounding after its input stops (capped).
    pub fn tail_seconds(&self) -> f64 {
        match self.tail {
            0 => 0.0,
            u32::MAX => 10.0,
            n => (f64::from(n) / self.setup.sample_rate.max(1.0)).min(30.0),
        }
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            if let Some(l) = self.links.lock().ok().and_then(|mut l| l.take()) {
                if let Some(p) = l.to_controller.as_com_ref::<IConnectionPoint>() {
                    l.component.disconnect(p.as_ptr());
                }
                if let Some(p) = l.to_component.as_com_ref::<IConnectionPoint>() {
                    l.controller.disconnect(p.as_ptr());
                }
            }
            if let Some(c) = &self.controller {
                if self.handler.is_some() {
                    c.setComponentHandler(ptr::null_mut());
                }
                if self.separate {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

/// Terminates a half-made plugin if loading fails part way.
struct Started {
    component: Option<ComPtr<IComponent>>,
    controller: Option<ComPtr<IEditController>>,
}

impl Drop for Started {
    fn drop(&mut self) {
        unsafe {
            if let Some(c) = self.controller.take() {
                c.terminate();
            }
            if let Some(c) = self.component.take() {
                c.terminate();
            }
        }
    }
}

/// Connect a plugin's two halves through UI-thread-only forwarders.
fn connect(component: &ComPtr<IComponent>, controller: &ComPtr<IEditController>) -> Option<Links> {
    let a = component.cast::<IConnectionPoint>()?;
    let b = controller.cast::<IConnectionPoint>()?;
    let ui = std::thread::current().id();
    let to_controller = ComWrapper::new(Link { to: b.clone(), ui });
    let to_component = ComWrapper::new(Link { to: a.clone(), ui });
    unsafe {
        a.connect(to_controller.as_com_ref::<IConnectionPoint>()?.as_ptr());
        b.connect(to_component.as_com_ref::<IConnectionPoint>()?.as_ptr());
    }
    Some(Links {
        component: a,
        controller: b,
        to_controller,
        to_component,
    })
}

/// Ask for stereo main buses, switch on the buses AIDAW uses, and read
/// back what the plugin settled on.
unsafe fn arrange(component: &ComPtr<IComponent>, processor: &ComPtr<IAudioProcessor>) -> Layout {
    use BusDirections_::{kInput, kOutput};
    use MediaTypes_::{kAudio, kEvent};
    let audio = kAudio as i32;
    let buses = |dir| unsafe { component.getBusCount(audio, dir as i32) }.clamp(0, 64) as usize;
    let info = |dir, i| {
        let mut b: BusInfo = unsafe { std::mem::zeroed() };
        (unsafe { component.getBusInfo(audio, dir as i32, i as i32, &mut b) } == kResultOk)
            .then_some(b)
    };
    let main = |dir| -> Option<usize> {
        let n = buses(dir);
        (0..n)
            .find(|&i| info(dir, i).is_some_and(|b| b.busType == BusTypes_::kMain as i32))
            .or((n > 0).then_some(0))
    };
    let (n_in, n_out) = (buses(kInput), buses(kOutput));
    let (main_in, main_out) = (main(kInput), main(kOutput));

    // Stereo on the main buses; the rest keep what they have.
    let current = |dir, i| {
        let mut arr: SpeakerArrangement = 0;
        if unsafe { processor.getBusArrangement(dir as i32, i as i32, &mut arr) } == kResultOk {
            arr
        } else {
            let ch = info(dir, i).map_or(0, |b| b.channelCount.clamp(0, 64));
            if ch >= 64 {
                u64::MAX
            } else {
                (1u64 << ch) - 1
            }
        }
    };
    let mut ins: Vec<SpeakerArrangement> = (0..n_in).map(|i| current(kInput, i)).collect();
    let mut outs: Vec<SpeakerArrangement> = (0..n_out).map(|i| current(kOutput, i)).collect();
    if let Some(m) = main_in {
        ins[m] = SpeakerArr::kStereo;
    }
    if let Some(m) = main_out {
        outs[m] = SpeakerArr::kStereo;
    }
    if unsafe {
        processor.setBusArrangements(
            ins.as_mut_ptr(),
            n_in as i32,
            outs.as_mut_ptr(),
            n_out as i32,
        )
    } != kResultTrue
    {
        // The plugin keeps its own layout (a mono effect, say); AIDAW
        // adapts to whatever it is.
    }

    for (dir, n, m) in [(kInput, n_in, main_in), (kOutput, n_out, main_out)] {
        for i in 0..n {
            unsafe { component.activateBus(audio, dir as i32, i as i32, (Some(i) == m) as u8) };
        }
    }
    let events = unsafe { component.getBusCount(kEvent as i32, kInput as i32) } > 0;
    if events {
        unsafe { component.activateBus(kEvent as i32, kInput as i32, 0, 1) };
    }
    let channels = |dir, n| -> Vec<i32> {
        (0..n)
            .map(|i| info(dir, i).map_or(0, |b| b.channelCount.clamp(0, 32)))
            .collect()
    };
    Layout {
        inputs: channels(kInput, n_in),
        outputs: channels(kOutput, n_out),
        main_in,
        main_out,
        events,
    }
}

/// The channel buffers of a plugin's buses in one direction.
struct Buses {
    channels: Vec<Vec<Vec<f32>>>,
    ptrs: Vec<Vec<*mut f32>>,
    buffers: Vec<AudioBusBuffers>,
}

impl Buses {
    fn new(counts: &[i32]) -> Buses {
        let mut channels: Vec<Vec<Vec<f32>>> = counts
            .iter()
            .map(|&n| (0..n.max(0)).map(|_| vec![0.0; MAX_BLOCK]).collect())
            .collect();
        let mut ptrs: Vec<Vec<*mut f32>> = channels
            .iter_mut()
            .map(|chs| chs.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        let buffers = ptrs
            .iter_mut()
            .map(|p| AudioBusBuffers {
                numChannels: p.len() as i32,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: if p.is_empty() {
                        ptr::null_mut()
                    } else {
                        p.as_mut_ptr()
                    },
                },
            })
            .collect();
        Buses {
            channels,
            ptrs,
            buffers,
        }
    }

    fn clear(&mut self, frames: usize) {
        for bus in &mut self.channels {
            for ch in bus {
                ch[..frames].fill(0.0);
            }
        }
        for b in &mut self.buffers {
            b.silenceFlags = 0;
        }
    }
}

/// The audio thread's side of a running plugin: everything one block
/// needs, made up front.
pub struct Processor {
    processor: ComPtr<IAudioProcessor>,
    mode: i32,
    events: ComWrapper<EventList>,
    changes_in: ComWrapper<ParameterChanges>,
    changes_out: ComWrapper<ParameterChanges>,
    inbox: Receiver<(ParamID, f64)>,
    outbox: SyncSender<(ParamID, f64)>,
    inputs: Buses,
    outputs: Buses,
    main_in: Option<usize>,
    main_out: Option<usize>,
    takes_notes: bool,
    context: ProcessContext,
}

// SAFETY: the raw pointers point into the buffers this struct owns, which
// never move or resize; the processor is used by one thread at a time.
unsafe impl Send for Processor {}

/// What a plugin does with the track's sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Plays notes, adding its sound to the bus.
    Instrument,
    /// Reshapes the bus.
    Effect,
}

impl Processor {
    fn new(
        processor: ComPtr<IAudioProcessor>,
        setup: Setup,
        layout: &Layout,
        inbox: Receiver<(ParamID, f64)>,
        outbox: SyncSender<(ParamID, f64)>,
    ) -> Processor {
        Processor {
            processor,
            mode: if setup.offline {
                ProcessModes_::kOffline as i32
            } else {
                ProcessModes_::kRealtime as i32
            },
            events: EventList::with_capacity(EVENTS),
            changes_in: ParameterChanges::with_capacity(CHANGES),
            changes_out: ParameterChanges::with_capacity(CHANGES),
            inbox,
            outbox,
            inputs: Buses::new(&layout.inputs),
            outputs: Buses::new(&layout.outputs),
            main_in: layout.main_in,
            main_out: layout.main_out,
            takes_notes: layout.events,
            context: unsafe { std::mem::zeroed() },
        }
    }

    /// Run one block of `frames` (at most [`MAX_BLOCK`]): as an instrument,
    /// play `events` and add the sound to `bus`; as an effect, run `bus`
    /// through the plugin. Never blocks or allocates.
    pub fn run(
        &mut self,
        role: Role,
        frames: usize,
        events: &[NoteEvent],
        transport: &Transport,
        bus: [&mut [f32]; 2],
    ) {
        let frames = frames.min(MAX_BLOCK).min(bus[0].len()).min(bus[1].len());
        if frames == 0 {
            return;
        }
        let [left, right] = bus;

        self.events.clear();
        if self.takes_notes {
            for e in events {
                self.events.push(note_event(e));
            }
        }
        for (id, v) in self.inbox.try_iter().take(INBOX) {
            self.changes_in.set(id, 0, v);
        }

        self.inputs.clear(frames);
        self.outputs.clear(frames);
        if role == Role::Effect {
            if let Some(m) = self.main_in {
                let chs = &mut self.inputs.channels[m];
                match chs.len() {
                    0 => {}
                    1 => {
                        for f in 0..frames {
                            chs[0][f] = 0.5 * (left[f] + right[f]);
                        }
                    }
                    _ => {
                        chs[0][..frames].copy_from_slice(&left[..frames]);
                        chs[1][..frames].copy_from_slice(&right[..frames]);
                    }
                }
            }
        }

        fill_context(&mut self.context, transport);
        let mut data = ProcessData {
            processMode: self.mode,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: frames as i32,
            numInputs: self.inputs.buffers.len() as i32,
            numOutputs: self.outputs.buffers.len() as i32,
            inputs: if self.inputs.buffers.is_empty() {
                ptr::null_mut()
            } else {
                self.inputs.buffers.as_mut_ptr()
            },
            outputs: if self.outputs.buffers.is_empty() {
                ptr::null_mut()
            } else {
                self.outputs.buffers.as_mut_ptr()
            },
            inputParameterChanges: host::changes_ptr(&self.changes_in),
            outputParameterChanges: host::changes_ptr(&self.changes_out),
            inputEvents: if self.takes_notes {
                host::events_ptr(&self.events)
            } else {
                ptr::null_mut()
            },
            outputEvents: ptr::null_mut(),
            processContext: &mut self.context,
        };
        // A plugin's error code says nothing the output doesn't: ignored.
        unsafe { self.processor.process(&mut data) };

        if let Some(m) = self.main_out {
            let chs = &self.outputs.channels[m];
            let (l, r) = match chs.len() {
                0 => (None, None),
                1 => (Some(&chs[0]), Some(&chs[0])),
                _ => (Some(&chs[0]), Some(&chs[1])),
            };
            if let (Some(l), Some(r)) = (l, r) {
                match role {
                    Role::Instrument => {
                        for f in 0..frames {
                            left[f] += clean(l[f]);
                            right[f] += clean(r[f]);
                        }
                    }
                    Role::Effect => {
                        for f in 0..frames {
                            left[f] = clean(l[f]);
                            right[f] = clean(r[f]);
                        }
                    }
                }
            }
        }

        for (id, v) in self.changes_out.last_values() {
            let _ = self.outbox.try_send((id, v));
        }
        self.changes_in.clear();
        self.changes_out.clear();
        self.events.clear();
        // Keep the pointers fresh in case the plugin wrote to them (some
        // swap buffers in place).
        for (b, p) in self.outputs.buffers.iter_mut().zip(&mut self.outputs.ptrs) {
            if !p.is_empty() {
                b.__field0.channelBuffers32 = p.as_mut_ptr();
            }
        }
    }
}

/// A plugin's sample, or silence if it is broken (NaN, infinite, absurd).
#[inline]
fn clean(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(-LIMIT, LIMIT)
    } else {
        0.0
    }
}

fn note_event(e: &NoteEvent) -> Event {
    let pitch = i16::from(e.pitch.min(127));
    let id = e.id & 0x7fff_ffff;
    Event {
        busIndex: 0,
        sampleOffset: e.offset as i32,
        ppqPosition: 0.0,
        flags: 0,
        r#type: if e.on {
            Event_::EventTypes_::kNoteOnEvent as u16
        } else {
            Event_::EventTypes_::kNoteOffEvent as u16
        },
        __field0: if e.on {
            Event__type0 {
                noteOn: NoteOnEvent {
                    channel: 0,
                    pitch,
                    tuning: 0.0,
                    velocity: e.velocity.clamp(0.0, 1.0),
                    length: 0,
                    noteId: id,
                },
            }
        } else {
            Event__type0 {
                noteOff: NoteOffEvent {
                    channel: 0,
                    pitch,
                    velocity: 0.0,
                    noteId: id,
                    tuning: 0.0,
                },
            }
        },
    }
}

/// The song's clock as VST3 describes it (beats are quarter notes).
fn fill_context(c: &mut ProcessContext, t: &Transport) {
    use ProcessContext_::StatesAndFlags_::*;
    let bpm = if t.bpm.is_finite() && t.bpm > 0.0 { t.bpm } else { 120.0 };
    let beat = if t.beat.is_finite() { t.beat.max(0.0) } else { 0.0 };
    let bpb = f64::from(t.beats_per_bar.max(1));
    let mut state = kTempoValid
        | kTimeSigValid
        | kProjectTimeMusicValid
        | kBarPositionValid
        | kContTimeValid;
    if t.playing {
        state |= kPlaying;
    }
    if t.loop_end.is_some() {
        state |= kCycleValid | kCycleActive;
    }
    c.state = state as u32;
    c.sampleRate = t.sample_rate;
    c.projectTimeSamples = (beat * 60.0 / bpm * t.sample_rate).round() as i64;
    c.systemTime = 0;
    c.continousTimeSamples = t.frames as i64;
    c.projectTimeMusic = beat;
    c.barPositionMusic = (beat / bpb).floor() * bpb;
    c.cycleStartMusic = 0.0;
    c.cycleEndMusic = t.loop_end.unwrap_or(0.0);
    c.tempo = bpm;
    c.timeSigNumerator = bpb as i32;
    c.timeSigDenominator = 4;
}

const _: () = {
    const fn sync<T: Send + Sync>() {}
    const fn send<T: Send>() {}
    sync::<Plugin>();
    send::<Processor>();
};
