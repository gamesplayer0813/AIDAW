//! A tiny VST®3 plugin module for testing AIDAW's plugin hosting without
//! installing anyone else's plugins. It holds two plugins:
//!
//! - "AIDAW Test Synth", an instrument: a sine per note, with a Level and an
//!   Octave parameter. Its editor half is a separate object that greets the
//!   audio half with a message through the host when they are connected.
//! - "AIDAW Test Gain", an effect: multiplies the sound by 0 to 2 (Gain
//!   parameter, 0.5 = unchanged). One object does everything.
//!
//! Build it with `cargo build -p aidaw-plugin --example test_plugin`; the
//! tests put the library into a `.vst3` bundle.
//!
//! VST is a registered trademark of Steinberg Media Technologies GmbH.

#![allow(non_snake_case)]

use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{uid, Class, ComPtr, ComRef, ComWrapper, Interface};

const SYNTH_CID: TUID = uid(0x41494441, 0x57000001, 0x54455354, 0x53594E54);
const SYNTH_CONTROLLER_CID: TUID = uid(0x41494441, 0x57000002, 0x54455354, 0x4354524C);
const GAIN_CID: TUID = uid(0x41494441, 0x57000003, 0x54455354, 0x4741494E);

const LEVEL: ParamID = 0;
const OCTAVE: ParamID = 1;
const GAIN: ParamID = 0;

fn copy_c(src: &str, dst: &mut [c_char]) {
    let n = src.len().min(dst.len() - 1);
    for (d, s) in dst.iter_mut().zip(src.bytes().take(n)) {
        *d = s as c_char;
    }
    dst[n] = 0;
}

/// Enum constants are C `int`s on Windows and `unsigned` elsewhere.
fn int(k: impl Into<i64>) -> i32 {
    k.into() as i32
}

fn uint(k: impl Into<i64>) -> u32 {
    k.into() as u32
}

fn copy_w(src: &str, dst: &mut [TChar]) {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s as TChar;
        n += 1;
    }
    let n = n.min(dst.len() - 1);
    dst[n] = 0;
}

struct F64(AtomicU64);

impl F64 {
    fn new(v: f64) -> F64 {
        F64(AtomicU64::new(v.to_bits()))
    }
    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

/// Write little-endian f64s to a host stream.
unsafe fn write_f64s(stream: *mut IBStream, values: &[f64]) -> tresult {
    let Some(s) = (unsafe { ComRef::from_raw(stream) }) else {
        return kInvalidArgument;
    };
    for v in values {
        let mut bytes = v.to_le_bytes();
        let mut n = 0;
        if unsafe { s.write(bytes.as_mut_ptr() as *mut c_void, 8, &mut n) } != kResultOk || n != 8 {
            return kResultFalse;
        }
    }
    kResultOk
}

/// Read up to `N` little-endian f64s from a host stream.
unsafe fn read_f64s<const N: usize>(stream: *mut IBStream) -> Option<[f64; N]> {
    let s = unsafe { ComRef::from_raw(stream) }?;
    let mut out = [0.0; N];
    for v in &mut out {
        let mut bytes = [0u8; 8];
        let mut n = 0;
        if unsafe { s.read(bytes.as_mut_ptr() as *mut c_void, 8, &mut n) } != kResultOk || n != 8 {
            return None;
        }
        *v = f64::from_le_bytes(bytes);
    }
    Some(out)
}

/// The last value of each parameter in a block's changes.
unsafe fn last_changes(changes: *mut IParameterChanges, mut f: impl FnMut(ParamID, f64)) {
    let Some(c) = (unsafe { ComRef::from_raw(changes) }) else {
        return;
    };
    for i in 0..unsafe { c.getParameterCount() } {
        let Some(q) = (unsafe { ComRef::from_raw(c.getParameterData(i)) }) else {
            continue;
        };
        let n = unsafe { q.getPointCount() };
        let (mut offset, mut value) = (0, 0.0);
        if n > 0 && unsafe { q.getPoint(n - 1, &mut offset, &mut value) } == kResultOk {
            f(unsafe { q.getParameterId() }, value);
        }
    }
}

fn bus_info(bus: *mut BusInfo, dir: i32, ch: i32, name: &str, media: i32) -> tresult {
    let Some(bus) = (unsafe { bus.as_mut() }) else {
        return kInvalidArgument;
    };
    bus.mediaType = media;
    bus.direction = dir;
    bus.channelCount = ch;
    copy_w(name, &mut bus.name);
    bus.busType = int(BusTypes_::kMain);
    bus.flags = uint(BusInfo_::BusFlags_::kDefaultActive);
    kResultOk
}

// ---- the synth's audio half ------------------------------------------------

#[derive(Clone, Copy)]
struct Voice {
    pitch: i16,
    id: i32,
    velocity: f32,
    phase: f64,
}

struct SynthProcessor {
    level: F64,
    octave: F64,
    sample_rate: F64,
    voices: Mutex<Vec<Voice>>,
    /// The editor half's greeting arrived.
    greeted: AtomicBool,
    peer: Mutex<Option<ComPtr<IConnectionPoint>>>,
}

impl Class for SynthProcessor {
    type Interfaces = (IComponent, IAudioProcessor, IConnectionPoint);
}

impl IPluginBaseTrait for SynthProcessor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        if let Ok(mut p) = self.peer.lock() {
            *p = None;
        }
        kResultOk
    }
}

impl IComponentTrait for SynthProcessor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        unsafe { *class_id = SYNTH_CONTROLLER_CID };
        kResultOk
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> int32 {
        match (media as u32, dir as u32) {
            (m, d) if m == uint(MediaTypes_::kAudio) && d == uint(BusDirections_::kOutput) => 1,
            (m, d) if m == uint(MediaTypes_::kEvent) && d == uint(BusDirections_::kInput) => 1,
            _ => 0,
        }
    }
    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: int32,
        bus: *mut BusInfo,
    ) -> tresult {
        if index != 0 || unsafe { self.getBusCount(media, dir) } == 0 {
            return kInvalidArgument;
        }
        if media == int(MediaTypes_::kAudio) {
            bus_info(bus, dir, 2, "Out", media)
        } else {
            bus_info(bus, dir, 16, "Notes", media)
        }
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: int32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        if let Ok(mut v) = self.voices.lock() {
            v.clear();
        }
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        match unsafe { read_f64s::<2>(state) } {
            Some([level, octave]) => {
                self.level.set(level);
                self.octave.set(octave);
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let greeted = if self.greeted.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        unsafe { write_f64s(state, &[self.level.get(), self.octave.get(), greeted]) }
    }
}

impl IAudioProcessorTrait for SynthProcessor {
    unsafe fn setBusArrangements(
        &self,
        _inputs: *mut SpeakerArrangement,
        num_ins: int32,
        outputs: *mut SpeakerArrangement,
        num_outs: int32,
    ) -> tresult {
        if num_ins == 0 && num_outs == 1 && unsafe { *outputs } == SpeakerArr::kStereo {
            kResultTrue
        } else {
            kResultFalse
        }
    }
    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: int32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if dir == int(BusDirections_::kOutput) && index == 0 {
            unsafe { *arr = SpeakerArr::kStereo };
            kResultOk
        } else {
            kInvalidArgument
        }
    }
    unsafe fn canProcessSampleSize(&self, size: int32) -> tresult {
        if size == int(SymbolicSampleSizes_::kSample32) {
            kResultOk
        } else {
            kNotImplemented
        }
    }
    unsafe fn getLatencySamples(&self) -> uint32 {
        0
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        self.sample_rate.set(unsafe { (*setup).sampleRate });
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = unsafe { &mut *data };
        unsafe {
            last_changes(data.inputParameterChanges, |id, v| match id {
                LEVEL => self.level.set(v),
                OCTAVE => self.octave.set(v),
                _ => {}
            })
        };
        let Ok(mut voices) = self.voices.try_lock() else {
            return kResultFalse;
        };
        let frames = data.numSamples.max(0) as usize;
        if data.numOutputs < 1 || data.outputs.is_null() {
            return kResultOk;
        }
        let out = unsafe { &mut *data.outputs };
        if out.numChannels < 2 {
            return kResultOk;
        }
        let chans = unsafe { std::slice::from_raw_parts(out.__field0.channelBuffers32, 2) };
        let (l, r) = unsafe {
            (
                std::slice::from_raw_parts_mut(chans[0], frames),
                std::slice::from_raw_parts_mut(chans[1], frames),
            )
        };
        let events = unsafe { ComRef::from_raw(data.inputEvents) };
        let n_events = events.as_ref().map_or(0, |e| unsafe { e.getEventCount() });
        let mut next = 0;
        let sr = self.sample_rate.get().max(1.0);
        let shift = (self.octave.get() * 2.0).round() - 1.0;
        let level = self.level.get() as f32;
        for f in 0..frames {
            while next < n_events {
                let mut e: Event = unsafe { std::mem::zeroed() };
                let Some(list) = &events else { break };
                if unsafe { list.getEvent(next, &mut e) } != kResultOk {
                    next += 1;
                    continue;
                }
                if e.sampleOffset as usize > f {
                    break;
                }
                next += 1;
                if e.r#type == Event_::EventTypes_::kNoteOnEvent as u16 {
                    let on = unsafe { e.__field0.noteOn };
                    if voices.len() < voices.capacity() {
                        voices.push(Voice {
                            pitch: on.pitch,
                            id: on.noteId,
                            velocity: on.velocity,
                            phase: 0.0,
                        });
                    }
                } else if e.r#type == Event_::EventTypes_::kNoteOffEvent as u16 {
                    let off = unsafe { e.__field0.noteOff };
                    if let Some(i) = voices
                        .iter()
                        .position(|v| v.id == off.noteId && v.pitch == off.pitch)
                        .or_else(|| voices.iter().position(|v| v.pitch == off.pitch))
                    {
                        voices.swap_remove(i);
                    }
                }
            }
            let mut s = 0.0f32;
            for v in voices.iter_mut() {
                let hz = 440.0 * 2f64.powf((f64::from(v.pitch) + 12.0 * shift - 69.0) / 12.0);
                s += (v.phase * std::f64::consts::TAU).sin() as f32 * v.velocity * level;
                v.phase = (v.phase + hz / sr).fract();
            }
            l[f] = s;
            r[f] = s;
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> uint32 {
        0
    }
}

impl IConnectionPointTrait for SynthProcessor {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        let peer = unsafe { ComRef::from_raw(other) }.map(|r| r.to_com_ptr());
        if let Ok(mut p) = self.peer.lock() {
            *p = peer;
        }
        kResultOk
    }
    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        if let Ok(mut p) = self.peer.lock() {
            *p = None;
        }
        kResultOk
    }
    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        let Some(m) = (unsafe { ComRef::from_raw(message) }) else {
            return kInvalidArgument;
        };
        let id = unsafe { m.getMessageID() };
        if !id.is_null() && unsafe { CStr::from_ptr(id) } == c"hello" {
            let mut n = 0;
            if let Some(attrs) = unsafe { ComRef::from_raw(m.getAttributes()) } {
                unsafe { attrs.getInt(c"n".as_ptr(), &mut n) };
            }
            self.greeted.store(n == 42, Ordering::Relaxed);
        }
        kResultOk
    }
}

// ---- the synth's editor half -----------------------------------------------

struct SynthController {
    level: F64,
    octave: F64,
    /// Only the editor half keeps this (its own settings).
    zoom: F64,
    host: Mutex<Option<ComPtr<IHostApplication>>>,
}

impl Class for SynthController {
    type Interfaces = (IEditController, IConnectionPoint);
}

impl IPluginBaseTrait for SynthController {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        let host = unsafe { ComRef::from_raw(context) }.and_then(|c| c.cast::<IHostApplication>());
        if let Ok(mut h) = self.host.lock() {
            *h = host;
        }
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        if let Ok(mut h) = self.host.lock() {
            *h = None;
        }
        kResultOk
    }
}

impl IEditControllerTrait for SynthController {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        match unsafe { read_f64s::<2>(state) } {
            Some([level, octave]) => {
                self.level.set(level);
                self.octave.set(octave);
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        match unsafe { read_f64s::<1>(state) } {
            Some([zoom]) => {
                self.zoom.set(zoom);
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        unsafe { write_f64s(state, &[self.zoom.get()]) }
    }
    unsafe fn getParameterCount(&self) -> int32 {
        2
    }
    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        let (id, name, steps, default) = match index {
            0 => (LEVEL, "Level", 0, 0.5),
            1 => (OCTAVE, "Octave", 2, 0.5),
            _ => return kInvalidArgument,
        };
        info.id = id;
        copy_w(name, &mut info.title);
        copy_w(name, &mut info.shortTitle);
        copy_w(if id == LEVEL { "%" } else { "" }, &mut info.units);
        info.stepCount = steps;
        info.defaultNormalizedValue = default;
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate
            | if id == OCTAVE {
                ParameterInfo_::ParameterFlags_::kIsList
            } else {
                0
            };
        kResultOk
    }
    unsafe fn getParamStringByValue(
        &self,
        id: ParamID,
        v: ParamValue,
        s: *mut String128,
    ) -> tresult {
        let Some(s) = (unsafe { s.as_mut() }) else {
            return kInvalidArgument;
        };
        let text = match id {
            LEVEL => format!("{:.0}", v * 100.0),
            OCTAVE => ["Low", "Middle", "High"][((v * 2.0).round() as usize).min(2)].to_string(),
            _ => return kInvalidArgument,
        };
        copy_w(&text, s);
        kResultOk
    }
    unsafe fn getParamValueByString(
        &self,
        _id: ParamID,
        _s: *mut TChar,
        _v: *mut ParamValue,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, _id: ParamID, v: ParamValue) -> ParamValue {
        v
    }
    unsafe fn plainParamToNormalized(&self, _id: ParamID, v: ParamValue) -> ParamValue {
        v
    }
    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        match id {
            LEVEL => self.level.get(),
            OCTAVE => self.octave.get(),
            _ => 0.0,
        }
    }
    unsafe fn setParamNormalized(&self, id: ParamID, v: ParamValue) -> tresult {
        match id {
            LEVEL => self.level.set(v),
            OCTAVE => self.octave.set(v),
            _ => return kInvalidArgument,
        }
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        ptr::null_mut()
    }
}

impl IConnectionPointTrait for SynthController {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        // Greet the audio half with a message made by the host.
        let Some(other) = (unsafe { ComRef::from_raw(other) }) else {
            return kInvalidArgument;
        };
        let host = self.host.lock().ok().and_then(|h| h.clone());
        if let Some(host) = host {
            let mut cid = IMessage::IID.map(|b| b as c_char);
            let mut iid = IMessage::IID.map(|b| b as c_char);
            let mut obj = ptr::null_mut();
            if unsafe { host.createInstance(&mut cid, &mut iid, &mut obj) } == kResultOk {
                if let Some(msg) = unsafe { ComPtr::from_raw(obj as *mut IMessage) } {
                    unsafe {
                        msg.setMessageID(c"hello".as_ptr());
                        if let Some(attrs) = ComRef::from_raw(msg.getAttributes()) {
                            attrs.setInt(c"n".as_ptr(), 42);
                        }
                        other.notify(msg.as_ptr());
                    }
                }
            }
        }
        kResultOk
    }
    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        kResultOk
    }
    unsafe fn notify(&self, _message: *mut IMessage) -> tresult {
        kResultOk
    }
}

// ---- the gain effect (one object) ------------------------------------------

struct Gain {
    gain: F64,
}

impl Class for Gain {
    type Interfaces = (IComponent, IAudioProcessor, IEditController);
}

impl IPluginBaseTrait for Gain {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Gain {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, _dir: BusDirection) -> int32 {
        (media == int(MediaTypes_::kAudio)) as int32
    }
    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: int32,
        bus: *mut BusInfo,
    ) -> tresult {
        if media != int(MediaTypes_::kAudio) || index != 0 {
            return kInvalidArgument;
        }
        bus_info(bus, dir, 2, "Main", media)
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: int32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        match unsafe { read_f64s::<1>(state) } {
            Some([g]) => {
                self.gain.set(g);
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        unsafe { write_f64s(state, &[self.gain.get()]) }
    }
}

impl IAudioProcessorTrait for Gain {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: int32,
        outputs: *mut SpeakerArrangement,
        num_outs: int32,
    ) -> tresult {
        let stereo = num_ins == 1
            && num_outs == 1
            && unsafe { *inputs == SpeakerArr::kStereo && *outputs == SpeakerArr::kStereo };
        if stereo {
            kResultTrue
        } else {
            kResultFalse
        }
    }
    unsafe fn getBusArrangement(
        &self,
        _dir: BusDirection,
        index: int32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = SpeakerArr::kStereo };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: int32) -> tresult {
        if size == int(SymbolicSampleSizes_::kSample32) {
            kResultOk
        } else {
            kNotImplemented
        }
    }
    unsafe fn getLatencySamples(&self) -> uint32 {
        0
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = unsafe { &mut *data };
        unsafe {
            last_changes(data.inputParameterChanges, |id, v| {
                if id == GAIN {
                    self.gain.set(v)
                }
            })
        };
        if data.numInputs < 1 || data.numOutputs < 1 {
            return kResultOk;
        }
        let frames = data.numSamples.max(0) as usize;
        let g = (self.gain.get() * 2.0) as f32;
        let (input, output) = unsafe { (&*data.inputs, &*data.outputs) };
        for ch in 0..2 {
            unsafe {
                let i =
                    std::slice::from_raw_parts(*input.__field0.channelBuffers32.add(ch), frames);
                let o = std::slice::from_raw_parts_mut(
                    *output.__field0.channelBuffers32.add(ch),
                    frames,
                );
                for f in 0..frames {
                    o[f] = i[f] * g;
                }
            }
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> uint32 {
        0
    }
}

impl IEditControllerTrait for Gain {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        unsafe { IComponentTrait::setState(self, state) }
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        unsafe { IComponentTrait::setState(self, state) }
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        unsafe { IComponentTrait::getState(self, state) }
    }
    unsafe fn getParameterCount(&self) -> int32 {
        1
    }
    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        if index != 0 {
            return kInvalidArgument;
        }
        info.id = GAIN;
        copy_w("Gain", &mut info.title);
        copy_w("Gain", &mut info.shortTitle);
        copy_w("x", &mut info.units);
        info.stepCount = 0;
        info.defaultNormalizedValue = 0.5;
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate;
        kResultOk
    }
    unsafe fn getParamStringByValue(
        &self,
        _id: ParamID,
        v: ParamValue,
        s: *mut String128,
    ) -> tresult {
        match unsafe { s.as_mut() } {
            Some(s) => {
                copy_w(&format!("{:.2}", v * 2.0), s);
                kResultOk
            }
            None => kInvalidArgument,
        }
    }
    unsafe fn getParamValueByString(
        &self,
        id: ParamID,
        s: *mut TChar,
        v: *mut ParamValue,
    ) -> tresult {
        // Reads what it shows: the factor, 0.00 to 2.00.
        if id != GAIN || s.is_null() || v.is_null() {
            return kInvalidArgument;
        }
        let mut len = 0;
        while unsafe { *s.add(len) } != 0 && len < 128 {
            len += 1;
        }
        let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(s, len) });
        match text.trim().parse::<f64>() {
            Ok(x) if (0.0..=2.0).contains(&x) => {
                unsafe { *v = x / 2.0 };
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn normalizedParamToPlain(&self, _id: ParamID, v: ParamValue) -> ParamValue {
        v * 2.0
    }
    unsafe fn plainParamToNormalized(&self, _id: ParamID, v: ParamValue) -> ParamValue {
        v / 2.0
    }
    unsafe fn getParamNormalized(&self, _id: ParamID) -> ParamValue {
        self.gain.get()
    }
    unsafe fn setParamNormalized(&self, id: ParamID, v: ParamValue) -> tresult {
        if id != GAIN {
            return kInvalidArgument;
        }
        self.gain.set(v);
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        ptr::null_mut()
    }
}

// ---- the factory -----------------------------------------------------------

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory3,);
}

struct Entry {
    cid: TUID,
    category: &'static str,
    name: &'static str,
    sub: &'static str,
}

const CLASSES: [Entry; 3] = [
    Entry {
        cid: SYNTH_CID,
        category: "Audio Module Class",
        name: "AIDAW Test Synth",
        sub: "Instrument|Synth",
    },
    Entry {
        cid: SYNTH_CONTROLLER_CID,
        category: "Component Controller Class",
        name: "AIDAW Test Synth",
        sub: "",
    },
    Entry {
        cid: GAIN_CID,
        category: "Audio Module Class",
        name: "AIDAW Test Gain",
        sub: "Fx|Dynamics",
    },
];

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        copy_c("AIDAW", &mut info.vendor);
        copy_c("https://github.com/gamesplayer0813/AIDAW", &mut info.url);
        copy_c("", &mut info.email);
        info.flags = int(PFactoryInfo_::FactoryFlags_::kUnicode);
        kResultOk
    }
    unsafe fn countClasses(&self) -> int32 {
        CLASSES.len() as int32
    }
    unsafe fn getClassInfo(&self, index: int32, info: *mut PClassInfo) -> tresult {
        let (Some(e), Some(info)) = (CLASSES.get(index as usize), unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        info.cid = e.cid;
        info.cardinality = int(PClassInfo_::ClassCardinality_::kManyInstances);
        copy_c(e.category, &mut info.category);
        copy_c(e.name, &mut info.name);
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        let cid = unsafe { *(cid as *const TUID) };
        let made: Option<ComPtr<FUnknown>> = if cid == SYNTH_CID {
            ComWrapper::new(SynthProcessor {
                level: F64::new(0.5),
                octave: F64::new(0.5),
                sample_rate: F64::new(44_100.0),
                voices: Mutex::new(Vec::with_capacity(64)),
                greeted: AtomicBool::new(false),
                peer: Mutex::new(None),
            })
            .to_com_ptr()
        } else if cid == SYNTH_CONTROLLER_CID {
            ComWrapper::new(SynthController {
                level: F64::new(0.5),
                octave: F64::new(0.5),
                zoom: F64::new(1.0),
                host: Mutex::new(None),
            })
            .to_com_ptr()
        } else if cid == GAIN_CID {
            ComWrapper::new(Gain {
                gain: F64::new(0.5),
            })
            .to_com_ptr()
        } else {
            None
        };
        match made {
            Some(p) => unsafe {
                let raw = p.as_ptr();
                ((*(*raw).vtbl).queryInterface)(raw, iid as *mut TUID, obj)
            },
            None => {
                unsafe { *obj = ptr::null_mut() };
                kNoInterface
            }
        }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: int32, info: *mut PClassInfo2) -> tresult {
        let (Some(e), Some(info)) = (CLASSES.get(index as usize), unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        info.cid = e.cid;
        info.cardinality = int(PClassInfo_::ClassCardinality_::kManyInstances);
        copy_c(e.category, &mut info.category);
        copy_c(e.name, &mut info.name);
        info.classFlags = 0;
        copy_c(e.sub, &mut info.subCategories);
        copy_c("AIDAW", &mut info.vendor);
        copy_c("1.0.0", &mut info.version);
        copy_c("VST 3.8.0", &mut info.sdkVersion);
        kResultOk
    }
}

impl IPluginFactory3Trait for Factory {
    unsafe fn getClassInfoUnicode(&self, index: int32, info: *mut PClassInfoW) -> tresult {
        let (Some(e), Some(info)) = (CLASSES.get(index as usize), unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        info.cid = e.cid;
        info.cardinality = int(PClassInfo_::ClassCardinality_::kManyInstances);
        copy_c(e.category, &mut info.category);
        copy_w(e.name, &mut info.name);
        info.classFlags = 0;
        copy_c(e.sub, &mut info.subCategories);
        copy_w("AIDAW", &mut info.vendor);
        copy_w("1.0.0", &mut info.version);
        copy_w("VST 3.8.0", &mut info.sdkVersion);
        kResultOk
    }
    unsafe fn setHostContext(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleEntry(_library_handle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleExit() -> bool {
    true
}

#[no_mangle]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .map_or(ptr::null_mut(), |p| p.into_raw())
}
