//! The objects a VST3 host hands to its plugins: who the host is, streams
//! for saving and loading state, messages between a plugin's two halves,
//! the note events and parameter changes of each audio block, and the
//! listener that hears about knob moves in the plugin's editor.

use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper, Interface};

use crate::util;

/// A class id as the bytes `Interface::IID` uses.
pub fn guid(tuid: &TUID) -> [u8; 16] {
    tuid.map(|b| b as u8)
}

/// The id of interface `I` in the form VST3 calls take.
pub fn iid<I: Interface>() -> TUID {
    I::IID.map(|b| b as std::ffi::c_char)
}

/// The host as plugins see it: its name, and a maker of messages for
/// plugins that ask the host for them.
pub struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication, IPlugInterfaceSupport);
}

/// The one host object every plugin gets.
pub fn host_application() -> &'static ComWrapper<HostApplication> {
    static HOST: OnceLock<ComWrapper<HostApplication>> = OnceLock::new();
    HOST.get_or_init(|| ComWrapper::new(HostApplication))
}

/// Raw `FUnknown` pointer to the host object (no reference is added; the
/// object lives for the whole run).
pub fn host_context() -> *mut FUnknown {
    host_application()
        .as_com_ref::<IHostApplication>()
        .map_or(ptr::null_mut(), |r| r.as_ptr() as *mut FUnknown)
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        util::to_utf16("AIDAW", unsafe { &mut *name });
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if cid.is_null() || iid.is_null() || obj.is_null() {
            return kInvalidArgument;
        }
        let (cid, iid) = unsafe { (guid(&*cid), guid(&*iid)) };
        let made = if cid == IMessage::IID && (iid == IMessage::IID || iid == FUnknown::IID) {
            ComWrapper::new(Message::new())
                .to_com_ptr::<IMessage>()
                .map(|p| p.into_raw() as *mut c_void)
        } else if cid == IAttributeList::IID && (iid == IAttributeList::IID || iid == FUnknown::IID)
        {
            ComWrapper::new(AttributeList::default())
                .to_com_ptr::<IAttributeList>()
                .map(|p| p.into_raw() as *mut c_void)
        } else {
            None
        };
        unsafe {
            match made {
                Some(p) => {
                    *obj = p;
                    kResultOk
                }
                None => {
                    *obj = ptr::null_mut();
                    kNoInterface
                }
            }
        }
    }
}

impl IPlugInterfaceSupportTrait for HostApplication {
    unsafe fn isPlugInterfaceSupported(&self, iid: *const TUID) -> tresult {
        if iid.is_null() {
            return kInvalidArgument;
        }
        let iid = guid(unsafe { &*iid });
        let supported = [
            IComponent::IID,
            IAudioProcessor::IID,
            IEditController::IID,
            IConnectionPoint::IID,
            IPlugView::IID,
            IPlugViewContentScaleSupport::IID,
            IProcessContextRequirements::IID,
        ];
        if supported.contains(&iid) {
            kResultTrue
        } else {
            kResultFalse
        }
    }
}

/// A message between a plugin's processor and its editor half.
pub struct Message {
    id: Mutex<Option<CString>>,
    attributes: ComWrapper<AttributeList>,
}

impl Message {
    pub fn new() -> Message {
        Message {
            id: Mutex::new(None),
            attributes: ComWrapper::new(AttributeList::default()),
        }
    }
}

impl Default for Message {
    fn default() -> Self {
        Message::new()
    }
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        // The pointer stays valid until the id is changed or the message freed.
        self.id
            .lock()
            .map(|id| id.as_ref().map_or(ptr::null(), |s| s.as_ptr()))
            .unwrap_or(ptr::null())
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        let id = unsafe { util::c_str_at(id) }.and_then(|s| CString::new(s).ok());
        if let Ok(mut slot) = self.id.lock() {
            *slot = id;
        }
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attributes
            .as_com_ref::<IAttributeList>()
            .map_or(ptr::null_mut(), |r| r.as_ptr())
    }
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<u16>),
    Bin(Box<[u8]>),
}

/// Named values attached to a [`Message`].
#[derive(Default)]
pub struct AttributeList {
    values: Mutex<HashMap<String, Attr>>,
}

impl Class for AttributeList {
    type Interfaces = (IAttributeList,);
}

impl AttributeList {
    fn set(&self, id: IAttributeList_::AttrID, value: Attr) -> tresult {
        let Some(key) = (unsafe { util::c_str_at(id) }) else {
            return kInvalidArgument;
        };
        match self.values.lock() {
            Ok(mut v) => {
                v.insert(key, value);
                kResultOk
            }
            Err(_) => kInternalError,
        }
    }

    fn get<T>(&self, id: IAttributeList_::AttrID, f: impl FnOnce(&Attr) -> Option<T>) -> Option<T> {
        let key = unsafe { util::c_str_at(id) }?;
        let values = self.values.lock().ok()?;
        f(values.get(&key)?)
    }
}

impl IAttributeListTrait for AttributeList {
    unsafe fn setInt(&self, id: IAttributeList_::AttrID, value: int64) -> tresult {
        self.set(id, Attr::Int(value))
    }

    unsafe fn getInt(&self, id: IAttributeList_::AttrID, value: *mut int64) -> tresult {
        match self.get(id, |a| match a {
            Attr::Int(v) => Some(*v),
            _ => None,
        }) {
            Some(v) if !value.is_null() => {
                unsafe { *value = v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: IAttributeList_::AttrID, value: f64) -> tresult {
        self.set(id, Attr::Float(value))
    }

    unsafe fn getFloat(&self, id: IAttributeList_::AttrID, value: *mut f64) -> tresult {
        match self.get(id, |a| match a {
            Attr::Float(v) => Some(*v),
            _ => None,
        }) {
            Some(v) if !value.is_null() => {
                unsafe { *value = v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: IAttributeList_::AttrID, string: *const TChar) -> tresult {
        match unsafe { util::utf16_at(string) } {
            Some(s) => self.set(id, Attr::Str(s)),
            None => kInvalidArgument,
        }
    }

    unsafe fn getString(
        &self,
        id: IAttributeList_::AttrID,
        string: *mut TChar,
        size_in_bytes: uint32,
    ) -> tresult {
        let chars = (size_in_bytes as usize) / 2;
        if string.is_null() || chars == 0 {
            return kInvalidArgument;
        }
        let found = self.get(id, |a| match a {
            Attr::Str(s) => {
                let n = s.len().min(chars - 1);
                unsafe {
                    ptr::copy_nonoverlapping(s.as_ptr(), string, n);
                    *string.add(n) = 0;
                }
                Some(())
            }
            _ => None,
        });
        if found.is_some() {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn setBinary(
        &self,
        id: IAttributeList_::AttrID,
        data: *const c_void,
        size_in_bytes: uint32,
    ) -> tresult {
        let bytes = if data.is_null() || size_in_bytes == 0 {
            Box::default()
        } else {
            unsafe { std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize) }.into()
        };
        self.set(id, Attr::Bin(bytes))
    }

    unsafe fn getBinary(
        &self,
        id: IAttributeList_::AttrID,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        if data.is_null() || size_in_bytes.is_null() {
            return kInvalidArgument;
        }
        // The bytes stay where they are until this attribute is replaced.
        let found = self.get(id, |a| match a {
            Attr::Bin(b) => Some((b.as_ptr() as *const c_void, b.len() as u32)),
            _ => None,
        });
        match found {
            Some((p, n)) => {
                unsafe {
                    *data = p;
                    *size_in_bytes = n;
                }
                kResultOk
            }
            None => kResultFalse,
        }
    }
}

/// A growable in-memory stream for plugin state.
#[derive(Default)]
pub struct MemoryStream {
    inner: Mutex<(Vec<u8>, usize)>,
}

impl Class for MemoryStream {
    type Interfaces = (IBStream, ISizeableStream);
}

impl MemoryStream {
    pub fn new() -> ComWrapper<MemoryStream> {
        ComWrapper::new(MemoryStream::default())
    }

    pub fn with_bytes(bytes: &[u8]) -> ComWrapper<MemoryStream> {
        ComWrapper::new(MemoryStream {
            inner: Mutex::new((bytes.to_vec(), 0)),
        })
    }

    /// Everything written so far.
    pub fn bytes(&self) -> Vec<u8> {
        self.inner.lock().map(|g| g.0.clone()).unwrap_or_default()
    }

    /// Move back to the start, to read what was written.
    pub fn rewind(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.1 = 0;
        }
    }
}

/// The `IBStream` pointer of a stream (no reference added).
pub fn stream_ptr(stream: &ComWrapper<MemoryStream>) -> *mut IBStream {
    stream
        .as_com_ref::<IBStream>()
        .map_or(ptr::null_mut(), |r| r.as_ptr())
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, read: *mut int32) -> tresult {
        let Ok(mut g) = self.inner.lock() else {
            return kInternalError;
        };
        let (data, pos) = &mut *g;
        let n = if buffer.is_null() || num_bytes <= 0 || *pos >= data.len() {
            0
        } else {
            (num_bytes as usize).min(data.len() - *pos)
        };
        if n > 0 {
            unsafe { ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buffer as *mut u8, n) };
            *pos += n;
        }
        if !read.is_null() {
            unsafe { *read = n as int32 };
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, written: *mut int32) -> tresult {
        let Ok(mut g) = self.inner.lock() else {
            return kInternalError;
        };
        let (data, pos) = &mut *g;
        let n = if buffer.is_null() || num_bytes <= 0 {
            0
        } else {
            num_bytes as usize
        };
        if n > 0 {
            if data.len() < *pos + n {
                data.resize(*pos + n, 0);
            }
            unsafe {
                ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(*pos), n)
            };
            *pos += n;
        }
        if !written.is_null() {
            unsafe { *written = n as int32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let Ok(mut g) = self.inner.lock() else {
            return kInternalError;
        };
        let base = match mode as u32 {
            m if m == util::uint(IBStream_::IStreamSeekMode_::kIBSeekSet) => 0,
            m if m == util::uint(IBStream_::IStreamSeekMode_::kIBSeekCur) => g.1 as i64,
            m if m == util::uint(IBStream_::IStreamSeekMode_::kIBSeekEnd) => g.0.len() as i64,
            _ => return kInvalidArgument,
        };
        let target = base.saturating_add(pos);
        if target < 0 {
            return kInvalidArgument;
        }
        g.1 = target as usize;
        if !result.is_null() {
            unsafe { *result = target };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        match self.inner.lock() {
            Ok(g) => {
                unsafe { *pos = g.1 as i64 };
                kResultOk
            }
            Err(_) => kInternalError,
        }
    }
}

impl ISizeableStreamTrait for MemoryStream {
    unsafe fn getStreamSize(&self, size: *mut int64) -> tresult {
        if size.is_null() {
            return kInvalidArgument;
        }
        match self.inner.lock() {
            Ok(g) => {
                unsafe { *size = g.0.len() as i64 };
                kResultOk
            }
            Err(_) => kInternalError,
        }
    }

    unsafe fn setStreamSize(&self, size: int64) -> tresult {
        if size < 0 {
            return kInvalidArgument;
        }
        match self.inner.lock() {
            Ok(mut g) => {
                g.0.resize(size as usize, 0);
                kResultOk
            }
            Err(_) => kInternalError,
        }
    }
}

/// What a plugin's editor told the host, shared with the UI.
pub struct Edits {
    /// Parameter moves on their way to the audio thread.
    pub to_processor: SyncSender<(ParamID, f64)>,
    /// Set by any edit; the UI clears it once it has saved the plugin's state.
    pub dirty: AtomicBool,
    /// Knob drags in progress (begin without end yet).
    pub gestures: AtomicI32,
    /// `restartComponent` flags the plugin asked for and nobody handled yet.
    pub restart: AtomicI32,
    /// The plugin asked the host to show its editor.
    pub open_editor: AtomicBool,
}

/// Hears about every knob the user moves in a plugin's own editor.
pub struct ComponentHandler {
    pub edits: Arc<Edits>,
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler, IComponentHandler2);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        self.edits.gestures.fetch_add(1, Ordering::AcqRel);
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        if value.is_finite() {
            // A full queue (the audio thread is stuck) drops the move; the
            // plugin's own state still has it.
            let _ = self
                .edits
                .to_processor
                .try_send((id, value.clamp(0.0, 1.0)));
        }
        self.edits.dirty.store(true, Ordering::Release);
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        let _ = self
            .edits
            .gestures
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |g| {
                Some((g - 1).max(0))
            });
        self.edits.dirty.store(true, Ordering::Release);
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.edits.restart.fetch_or(flags, Ordering::AcqRel);
        if flags & (RestartFlags_::kParamValuesChanged | RestartFlags_::kReloadComponent) != 0 {
            self.edits.dirty.store(true, Ordering::Release);
        }
        kResultOk
    }
}

impl IComponentHandler2Trait for ComponentHandler {
    unsafe fn setDirty(&self, state: TBool) -> tresult {
        if state != 0 {
            self.edits.dirty.store(true, Ordering::Release);
        }
        kResultOk
    }

    unsafe fn requestOpenEditor(&self, _name: FIDString) -> tresult {
        self.edits.open_editor.store(true, Ordering::Release);
        kResultOk
    }

    unsafe fn startGroupEdit(&self) -> tresult {
        kResultOk
    }

    unsafe fn finishGroupEdit(&self) -> tresult {
        kResultOk
    }
}

/// The note events of one block. Only the thread running the plugin's
/// `process` touches it, so it needs no lock; its room is reserved up front.
pub struct EventList {
    events: UnsafeCell<Vec<Event>>,
}

// SAFETY: used by one thread at a time (the one calling `process`, which
// the plugin wrapper serialises).
unsafe impl Send for EventList {}
unsafe impl Sync for EventList {}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl EventList {
    pub fn with_capacity(n: usize) -> ComWrapper<EventList> {
        ComWrapper::new(EventList {
            events: UnsafeCell::new(Vec::with_capacity(n)),
        })
    }

    #[allow(clippy::mut_from_ref)]
    fn list(&self) -> &mut Vec<Event> {
        // SAFETY: see the type's documentation.
        unsafe { &mut *self.events.get() }
    }

    pub fn clear(&self) {
        self.list().clear();
    }

    /// Add an event if there is room (never allocates).
    pub fn push(&self, e: Event) -> bool {
        let list = self.list();
        if list.len() < list.capacity() {
            list.push(e);
            true
        } else {
            false
        }
    }

    pub fn len(&self) -> usize {
        self.list().len()
    }

    pub fn is_empty(&self) -> bool {
        self.list().is_empty()
    }
}

/// The `IEventList` pointer of an event list (no reference added).
pub fn events_ptr(list: &ComWrapper<EventList>) -> *mut IEventList {
    list.as_com_ref::<IEventList>()
        .map_or(ptr::null_mut(), |r| r.as_ptr())
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.list().len() as int32
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        match (
            usize::try_from(index).ok().and_then(|i| self.list().get(i)),
            e.is_null(),
        ) {
            (Some(ev), false) => {
                unsafe { *e = *ev };
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        if e.is_null() {
            return kInvalidArgument;
        }
        if self.push(unsafe { *e }) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

/// Points of one parameter's changes in a block (room reserved up front).
pub struct ParamValueQueue {
    id: Cell<ParamID>,
    points: UnsafeCell<Vec<(int32, f64)>>,
}

// SAFETY: as for EventList.
unsafe impl Send for ParamValueQueue {}
unsafe impl Sync for ParamValueQueue {}

impl Class for ParamValueQueue {
    type Interfaces = (IParamValueQueue,);
}

/// Points kept per parameter per block.
const POINTS: usize = 32;

impl ParamValueQueue {
    #[allow(clippy::mut_from_ref)]
    fn points(&self) -> &mut Vec<(int32, f64)> {
        // SAFETY: see EventList.
        unsafe { &mut *self.points.get() }
    }

    /// Add a point in time order; a point on the same frame is replaced.
    fn add(&self, offset: int32, value: f64) -> Option<usize> {
        let points = self.points();
        match points.binary_search_by_key(&offset, |p| p.0) {
            Ok(i) => {
                points[i].1 = value;
                Some(i)
            }
            Err(i) if points.len() < points.capacity() => {
                points.insert(i, (offset, value));
                Some(i)
            }
            Err(_) => {
                // Full: the latest value wins over the last point.
                let last = points.len() - 1;
                points[last].1 = value;
                Some(last)
            }
        }
    }

    pub fn last_value(&self) -> Option<f64> {
        self.points().last().map(|p| p.1)
    }
}

impl IParamValueQueueTrait for ParamValueQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> int32 {
        self.points().len() as int32
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        let Some(&(o, v)) = usize::try_from(index)
            .ok()
            .and_then(|i| self.points().get(i))
        else {
            return kInvalidArgument;
        };
        unsafe {
            if !offset.is_null() {
                *offset = o;
            }
            if !value.is_null() {
                *value = v;
            }
        }
        kResultOk
    }

    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        match self.add(offset, value) {
            Some(i) => {
                if !index.is_null() {
                    unsafe { *index = i as int32 };
                }
                kResultOk
            }
            None => kResultFalse,
        }
    }
}

/// The parameter changes of one block, for or from a plugin.
pub struct ParameterChanges {
    queues: Vec<ComWrapper<ParamValueQueue>>,
    used: Cell<usize>,
}

// SAFETY: as for EventList.
unsafe impl Send for ParameterChanges {}
unsafe impl Sync for ParameterChanges {}

impl Class for ParameterChanges {
    type Interfaces = (IParameterChanges,);
}

impl ParameterChanges {
    pub fn with_capacity(n: usize) -> ComWrapper<ParameterChanges> {
        ComWrapper::new(ParameterChanges {
            queues: (0..n)
                .map(|_| {
                    ComWrapper::new(ParamValueQueue {
                        id: Cell::new(0),
                        points: UnsafeCell::new(Vec::with_capacity(POINTS)),
                    })
                })
                .collect(),
            used: Cell::new(0),
        })
    }

    pub fn clear(&self) {
        for q in &self.queues[..self.used.get()] {
            q.points().clear();
        }
        self.used.set(0);
    }

    pub fn is_empty(&self) -> bool {
        self.used.get() == 0
    }

    /// The queue for `id`, opening one if there is room.
    fn queue(&self, id: ParamID) -> Option<(usize, &ComWrapper<ParamValueQueue>)> {
        let used = self.used.get();
        if let Some(i) = self.queues[..used].iter().position(|q| q.id.get() == id) {
            return Some((i, &self.queues[i]));
        }
        let q = self.queues.get(used)?;
        q.id.set(id);
        q.points().clear();
        self.used.set(used + 1);
        Some((used, q))
    }

    /// Record that parameter `id` takes `value` at frame `offset`.
    pub fn set(&self, id: ParamID, offset: i32, value: f64) -> bool {
        match self.queue(id) {
            Some((_, q)) => q.add(offset, value).is_some(),
            None => false,
        }
    }

    /// Each changed parameter and its last value.
    pub fn last_values(&self) -> impl Iterator<Item = (ParamID, f64)> + '_ {
        self.queues[..self.used.get()]
            .iter()
            .filter_map(|q| Some((q.id.get(), q.last_value()?)))
    }
}

/// The `IParameterChanges` pointer of a change list (no reference added).
pub fn changes_ptr(changes: &ComWrapper<ParameterChanges>) -> *mut IParameterChanges {
    changes
        .as_com_ref::<IParameterChanges>()
        .map_or(ptr::null_mut(), |r| r.as_ptr())
}

impl IParameterChangesTrait for ParameterChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.used.get() as int32
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        // Like the SDK's own host: the list keeps ownership, no reference added.
        usize::try_from(index)
            .ok()
            .filter(|i| *i < self.used.get())
            .and_then(|i| self.queues[i].as_com_ref::<IParamValueQueue>())
            .map_or(ptr::null_mut(), |r| r.as_ptr())
    }

    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        if id.is_null() {
            return ptr::null_mut();
        }
        match self.queue(unsafe { *id }) {
            Some((i, q)) => {
                if !index.is_null() {
                    unsafe { *index = i as int32 };
                }
                q.as_com_ref::<IParamValueQueue>()
                    .map_or(ptr::null_mut(), |r| r.as_ptr())
            }
            None => ptr::null_mut(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3::{ComPtr, ComRef};

    #[test]
    fn memory_streams_read_write_and_seek() {
        let s = MemoryStream::new();
        let p = unsafe { ComRef::from_raw(stream_ptr(&s)) }.unwrap();
        let mut n = 0;
        unsafe {
            assert_eq!(
                p.write(b"hello".as_ptr() as *mut c_void, 5, &mut n),
                kResultOk
            );
            assert_eq!(n, 5);
            let mut at = 0;
            p.seek(
                1,
                util::int(IBStream_::IStreamSeekMode_::kIBSeekSet),
                &mut at,
            );
            assert_eq!(at, 1);
            let mut buf = [0u8; 10];
            p.read(buf.as_mut_ptr() as *mut c_void, 10, &mut n);
            assert_eq!(&buf[..n as usize], b"ello");
            p.read(buf.as_mut_ptr() as *mut c_void, 10, &mut n);
            assert_eq!(n, 0, "at the end");
            p.seek(
                -2,
                util::int(IBStream_::IStreamSeekMode_::kIBSeekEnd),
                &mut at,
            );
            p.write(b"LP".as_ptr() as *mut c_void, 2, &mut n);
        }
        assert_eq!(s.bytes(), b"helLP");
    }

    #[test]
    fn parameter_changes_keep_points_in_order_without_growing() {
        let c = ParameterChanges::with_capacity(2);
        assert!(c.set(7, 10, 0.5));
        assert!(c.set(7, 0, 0.25));
        assert!(c.set(7, 10, 0.75));
        assert!(c.set(9, 3, 1.0));
        assert!(!c.set(11, 0, 1.0), "no room for a third parameter");
        let p = unsafe { ComRef::from_raw(changes_ptr(&c)) }.unwrap();
        unsafe {
            assert_eq!(p.getParameterCount(), 2);
            let q = ComRef::from_raw(p.getParameterData(0)).unwrap();
            assert_eq!(q.getParameterId(), 7);
            assert_eq!(q.getPointCount(), 2);
            let (mut o, mut v) = (0, 0.0);
            q.getPoint(1, &mut o, &mut v);
            assert_eq!((o, v), (10, 0.75));
            assert!(p.getParameterData(2).is_null());
        }
        assert_eq!(
            c.last_values().collect::<Vec<_>>(),
            vec![(7, 0.75), (9, 1.0)]
        );
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn the_host_makes_messages_with_attributes() {
        let host = host_application().to_com_ptr::<IHostApplication>().unwrap();
        let mut cid = iid::<IMessage>();
        let mut want = iid::<IMessage>();
        let mut obj = ptr::null_mut();
        unsafe {
            assert_eq!(
                host.createInstance(&mut cid, &mut want, &mut obj),
                kResultOk
            );
            let msg = ComPtr::from_raw(obj as *mut IMessage).unwrap();
            msg.setMessageID(c"hello".as_ptr());
            assert_eq!(util::c_str_at(msg.getMessageID()).as_deref(), Some("hello"));
            let attrs = ComRef::from_raw(msg.getAttributes()).unwrap();
            attrs.setInt(c"n".as_ptr(), 42);
            let mut n = 0;
            assert_eq!(attrs.getInt(c"n".as_ptr(), &mut n), kResultOk);
            assert_eq!(n, 42);
            let data = [1u8, 2, 3];
            attrs.setBinary(c"b".as_ptr(), data.as_ptr() as *const c_void, 3);
            let (mut p, mut len) = (ptr::null(), 0);
            attrs.getBinary(c"b".as_ptr(), &mut p, &mut len);
            assert_eq!(
                std::slice::from_raw_parts(p as *const u8, len as usize),
                &data
            );
            let text: Vec<u16> = "hey\0".encode_utf16().collect();
            attrs.setString(c"s".as_ptr(), text.as_ptr());
            let mut out = [0u16; 8];
            attrs.getString(c"s".as_ptr(), out.as_mut_ptr(), 16);
            assert_eq!(util::from_utf16(&out), "hey");
            let mut name = [0u16; 128];
            host.getName(&mut name);
            assert_eq!(util::from_utf16(&name), "AIDAW");
        }
    }
}
