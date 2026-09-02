//! Drag-out module: extracts archive entries into a temporary staging directory
//! and initiates native Windows OLE `DoDragDrop` with a `CF_HDROP` data object.

use crate::extraction::{extract_any, FailOnConflict};
use crate::models::CommandError;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
type HRESULT = i32;

#[cfg(windows)]
const S_OK: HRESULT = 0;
#[cfg(windows)]
const S_FALSE: HRESULT = 1;
#[cfg(windows)]
const DATA_S_SAMEFORMATETC: HRESULT = 0x0004_0130;
#[cfg(windows)]
const DRAGDROP_S_DROP: HRESULT = 0x0004_0100;
#[cfg(windows)]
const DRAGDROP_S_CANCEL: HRESULT = 0x0004_0101;
#[cfg(windows)]
const DRAGDROP_S_USEDEFAULTCURSORS: HRESULT = 0x0004_0102;

#[cfg(windows)]
const E_NOTIMPL: HRESULT = -2147467263; // 0x80004001
#[cfg(windows)]
const E_NOINTERFACE: HRESULT = -2147467262; // 0x80004002
#[cfg(windows)]
const E_POINTER: HRESULT = -2147467261; // 0x80004003
#[cfg(windows)]
const E_INVALIDARG: HRESULT = -2147024809; // 0x80070057
#[cfg(windows)]
const E_OUTOFMEMORY: HRESULT = -2147024882; // 0x8007000E
#[cfg(windows)]
const E_UNEXPECTED: HRESULT = -2147418113; // 0x8000FFFF
#[cfg(windows)]
const DV_E_FORMATETC: HRESULT = -2147221404; // 0x80040064

#[cfg(windows)]
const CF_HDROP: u16 = 15;
#[cfg(windows)]
const TYMED_HGLOBAL: u32 = 1;
#[cfg(windows)]
const DATADIR_GET: u32 = 1;
#[cfg(windows)]
const DROPEFFECT_COPY: u32 = 1;
#[cfg(windows)]
const DROPEFFECT_MOVE: u32 = 2;
#[cfg(windows)]
const DROPEFFECT_LINK: u32 = 4;

#[cfg(windows)]
const MK_LBUTTON: u32 = 0x0001;
#[cfg(windows)]
const MK_RBUTTON: u32 = 0x0002;
#[cfg(windows)]
const GMEM_MOVEABLE: u32 = 0x0002;
#[cfg(windows)]
const GMEM_ZEROINIT: u32 = 0x0040;

#[cfg(windows)]
const VK_LBUTTON: i32 = 0x01;
#[cfg(windows)]
const VK_RBUTTON: i32 = 0x02;

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

#[cfg(windows)]
const IID_IUNKNOWN: GUID = GUID {
    data1: 0x00000000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[cfg(windows)]
const IID_IDATAOBJECT: GUID = GUID {
    data1: 0x0000010E,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[cfg(windows)]
const IID_IDROPSOURCE: GUID = GUID {
    data1: 0x00000121,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[cfg(windows)]
const IID_IENUMFORMATETC: GUID = GUID {
    data1: 0x00000103,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[cfg(windows)]
#[repr(C)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
pub struct DROPFILES {
    pub pFiles: u32,
    pub pt: POINT,
    pub fNC: i32,
    pub fWide: i32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
pub struct FORMATETC {
    pub cfFormat: u16,
    pub ptd: *mut c_void,
    pub dwAspect: u32,
    pub lindex: i32,
    pub tymed: u32,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
pub struct STGMEDIUM {
    pub tymed: u32,
    pub hGlobal: *mut c_void,
    pub pUnkForRelease: *mut c_void,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
pub struct IDataObjectVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub GetData:
        unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut STGMEDIUM) -> HRESULT,
    pub GetDataHere:
        unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut STGMEDIUM) -> HRESULT,
    pub QueryGetData: unsafe extern "system" fn(*mut c_void, *const FORMATETC) -> HRESULT,
    pub GetCanonicalFormatEtc:
        unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut FORMATETC) -> HRESULT,
    pub SetData:
        unsafe extern "system" fn(*mut c_void, *const FORMATETC, *const STGMEDIUM, i32) -> HRESULT,
    pub EnumFormatEtc: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    pub DAdvise: unsafe extern "system" fn(
        *mut c_void,
        *const FORMATETC,
        u32,
        *mut c_void,
        *mut u32,
    ) -> HRESULT,
    pub DUnadvise: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    pub EnumDAdvise: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
pub struct IDropSourceVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub QueryContinueDrag: unsafe extern "system" fn(*mut c_void, i32, u32) -> HRESULT,
    pub GiveFeedback: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
pub struct IEnumFORMATETCVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub Next: unsafe extern "system" fn(*mut c_void, u32, *mut FORMATETC, *mut u32) -> HRESULT,
    pub Skip: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    pub Reset: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    pub Clone: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
}

#[cfg(windows)]
#[link(name = "ole32")]
extern "system" {
    fn OleInitialize(reserved: *mut c_void) -> HRESULT;
    fn OleUninitialize();
    fn DoDragDrop(
        pDataObj: *mut c_void,
        pDropSource: *mut c_void,
        dwOKEffects: u32,
        pdwEffect: *mut u32,
    ) -> HRESULT;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> *mut c_void;
    fn GlobalLock(hMem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(hMem: *mut c_void) -> i32;
    fn GlobalFree(hMem: *mut c_void) -> *mut c_void;
    fn GlobalSize(hMem: *mut c_void) -> usize;
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vKey: i32) -> i16;
}

#[cfg(windows)]
pub fn is_lbutton_pressed() -> bool {
    unsafe { (GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000) != 0 }
}

#[cfg(not(windows))]
pub fn is_lbutton_pressed() -> bool {
    false
}

#[cfg(windows)]
#[repr(C)]
struct FileDataObject {
    vtbl: &'static IDataObjectVtbl,
    ref_count: AtomicU32,
    hglobal: *mut c_void,
}

#[cfg(windows)]
#[repr(C)]
struct DropSource {
    vtbl: &'static IDropSourceVtbl,
    ref_count: AtomicU32,
}

#[cfg(windows)]
#[repr(C)]
struct EnumFormatEtc {
    vtbl: &'static IEnumFORMATETCVtbl,
    ref_count: AtomicU32,
    index: usize,
    formats: Vec<FORMATETC>,
}

#[cfg(windows)]
unsafe extern "system" fn data_object_query_interface(
    this: *mut c_void,
    riid: *const GUID,
    ppv_object: *mut *mut c_void,
) -> HRESULT {
    if riid.is_null() || ppv_object.is_null() {
        return E_POINTER;
    }
    let guid = &*riid;
    if *guid == IID_IUNKNOWN || *guid == IID_IDATAOBJECT {
        *ppv_object = this;
        data_object_add_ref(this);
        S_OK
    } else {
        *ppv_object = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

#[cfg(windows)]
unsafe extern "system" fn data_object_add_ref(this: *mut c_void) -> u32 {
    let obj = &*(this as *mut FileDataObject);
    obj.ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

#[cfg(windows)]
unsafe extern "system" fn data_object_release(this: *mut c_void) -> u32 {
    let obj = &*(this as *mut FileDataObject);
    let count = obj.ref_count.fetch_sub(1, Ordering::SeqCst) - 1;
    if count == 0 {
        if !obj.hglobal.is_null() {
            GlobalFree(obj.hglobal);
        }
        let _ = Box::from_raw(this as *mut FileDataObject);
    }
    count
}

#[cfg(windows)]
unsafe extern "system" fn data_object_get_data(
    this: *mut c_void,
    pformatetc: *const FORMATETC,
    pmedium: *mut STGMEDIUM,
) -> HRESULT {
    if pformatetc.is_null() || pmedium.is_null() {
        return E_INVALIDARG;
    }
    let format = &*pformatetc;
    if format.cfFormat != CF_HDROP || (format.tymed & TYMED_HGLOBAL) == 0 {
        return DV_E_FORMATETC;
    }
    let obj = &*(this as *mut FileDataObject);
    if obj.hglobal.is_null() {
        return E_UNEXPECTED;
    }
    let size = GlobalSize(obj.hglobal);
    if size == 0 {
        return E_OUTOFMEMORY;
    }
    let new_hglobal = GlobalAlloc(GMEM_MOVEABLE, size);
    if new_hglobal.is_null() {
        return E_OUTOFMEMORY;
    }
    let src_ptr = GlobalLock(obj.hglobal);
    let dst_ptr = GlobalLock(new_hglobal);
    if src_ptr.is_null() || dst_ptr.is_null() {
        if !src_ptr.is_null() {
            GlobalUnlock(obj.hglobal);
        }
        if !dst_ptr.is_null() {
            GlobalUnlock(new_hglobal);
        }
        GlobalFree(new_hglobal);
        return E_OUTOFMEMORY;
    }
    std::ptr::copy_nonoverlapping(src_ptr as *const u8, dst_ptr as *mut u8, size);
    GlobalUnlock(obj.hglobal);
    GlobalUnlock(new_hglobal);

    let med = &mut *pmedium;
    med.tymed = TYMED_HGLOBAL;
    med.hGlobal = new_hglobal;
    med.pUnkForRelease = std::ptr::null_mut();
    S_OK
}

#[cfg(windows)]
unsafe extern "system" fn data_object_get_data_here(
    _this: *mut c_void,
    _pformatetc: *const FORMATETC,
    _pmedium: *mut STGMEDIUM,
) -> HRESULT {
    E_NOTIMPL
}

#[cfg(windows)]
unsafe extern "system" fn data_object_query_get_data(
    _this: *mut c_void,
    pformatetc: *const FORMATETC,
) -> HRESULT {
    if pformatetc.is_null() {
        return E_INVALIDARG;
    }
    let format = &*pformatetc;
    if format.cfFormat == CF_HDROP && (format.tymed & TYMED_HGLOBAL) != 0 {
        S_OK
    } else {
        DV_E_FORMATETC
    }
}

#[cfg(windows)]
unsafe extern "system" fn data_object_get_canonical_format_etc(
    _this: *mut c_void,
    pformatetc_in: *const FORMATETC,
    pformatetc_out: *mut FORMATETC,
) -> HRESULT {
    if pformatetc_out.is_null() {
        return E_INVALIDARG;
    }
    if !pformatetc_in.is_null() {
        let in_ref = &*pformatetc_in;
        let out_ref = &mut *pformatetc_out;
        *out_ref = *in_ref;
        out_ref.ptd = std::ptr::null_mut();
    }
    DATA_S_SAMEFORMATETC
}

#[cfg(windows)]
unsafe extern "system" fn data_object_set_data(
    _this: *mut c_void,
    _pformatetc: *const FORMATETC,
    _pmedium: *const STGMEDIUM,
    _f_release: i32,
) -> HRESULT {
    E_NOTIMPL
}

#[cfg(windows)]
unsafe extern "system" fn data_object_enum_format_etc(
    _this: *mut c_void,
    dw_direction: u32,
    pp_enum: *mut *mut c_void,
) -> HRESULT {
    if pp_enum.is_null() {
        return E_POINTER;
    }
    if dw_direction == DATADIR_GET {
        let enum_obj = Box::into_raw(Box::new(EnumFormatEtc {
            vtbl: &ENUM_FORMAT_ETC_VTBL,
            ref_count: AtomicU32::new(1),
            index: 0,
            formats: vec![FORMATETC {
                cfFormat: CF_HDROP,
                ptd: std::ptr::null_mut(),
                dwAspect: 1, // DVASPECT_CONTENT
                lindex: -1,
                tymed: TYMED_HGLOBAL,
            }],
        }));
        *pp_enum = enum_obj as *mut c_void;
        S_OK
    } else {
        *pp_enum = std::ptr::null_mut();
        E_NOTIMPL
    }
}

#[cfg(windows)]
unsafe extern "system" fn data_object_dadvise(
    _this: *mut c_void,
    _pformatetc: *const FORMATETC,
    _advf: u32,
    _p_adv_sink: *mut c_void,
    pdw_connection: *mut u32,
) -> HRESULT {
    if !pdw_connection.is_null() {
        *pdw_connection = 0;
    }
    E_NOTIMPL
}

#[cfg(windows)]
unsafe extern "system" fn data_object_dunadvise(
    _this: *mut c_void,
    _dw_connection: u32,
) -> HRESULT {
    E_NOTIMPL
}

#[cfg(windows)]
unsafe extern "system" fn data_object_enum_dadvise(
    _this: *mut c_void,
    pp_enum_advise: *mut *mut c_void,
) -> HRESULT {
    if !pp_enum_advise.is_null() {
        *pp_enum_advise = std::ptr::null_mut();
    }
    E_NOTIMPL
}

#[cfg(windows)]
static DATA_OBJECT_VTBL: IDataObjectVtbl = IDataObjectVtbl {
    QueryInterface: data_object_query_interface,
    AddRef: data_object_add_ref,
    Release: data_object_release,
    GetData: data_object_get_data,
    GetDataHere: data_object_get_data_here,
    QueryGetData: data_object_query_get_data,
    GetCanonicalFormatEtc: data_object_get_canonical_format_etc,
    SetData: data_object_set_data,
    EnumFormatEtc: data_object_enum_format_etc,
    DAdvise: data_object_dadvise,
    DUnadvise: data_object_dunadvise,
    EnumDAdvise: data_object_enum_dadvise,
};

#[cfg(windows)]
unsafe extern "system" fn drop_source_query_interface(
    this: *mut c_void,
    riid: *const GUID,
    ppv_object: *mut *mut c_void,
) -> HRESULT {
    if riid.is_null() || ppv_object.is_null() {
        return E_POINTER;
    }
    let guid = &*riid;
    if *guid == IID_IUNKNOWN || *guid == IID_IDROPSOURCE {
        *ppv_object = this;
        drop_source_add_ref(this);
        S_OK
    } else {
        *ppv_object = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

#[cfg(windows)]
unsafe extern "system" fn drop_source_add_ref(this: *mut c_void) -> u32 {
    let obj = &*(this as *mut DropSource);
    obj.ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

#[cfg(windows)]
unsafe extern "system" fn drop_source_release(this: *mut c_void) -> u32 {
    let obj = &*(this as *mut DropSource);
    let count = obj.ref_count.fetch_sub(1, Ordering::SeqCst) - 1;
    if count == 0 {
        let _ = Box::from_raw(this as *mut DropSource);
    }
    count
}

#[cfg(windows)]
unsafe extern "system" fn drop_source_query_continue_drag(
    _this: *mut c_void,
    f_escape_pressed: i32,
    grf_key_state: u32,
) -> HRESULT {
    if f_escape_pressed != 0 {
        return DRAGDROP_S_CANCEL;
    }
    let l_down = (GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000) != 0;
    let r_down = (GetAsyncKeyState(VK_RBUTTON) as u16 & 0x8000) != 0;
    let grf_down = (grf_key_state & (MK_LBUTTON | MK_RBUTTON)) != 0;

    if !l_down && !r_down && !grf_down {
        return DRAGDROP_S_DROP;
    }
    S_OK
}

#[cfg(windows)]
unsafe extern "system" fn drop_source_give_feedback(
    _this: *mut c_void,
    _dw_effect: u32,
) -> HRESULT {
    DRAGDROP_S_USEDEFAULTCURSORS
}

#[cfg(windows)]
static DROP_SOURCE_VTBL: IDropSourceVtbl = IDropSourceVtbl {
    QueryInterface: drop_source_query_interface,
    AddRef: drop_source_add_ref,
    Release: drop_source_release,
    QueryContinueDrag: drop_source_query_continue_drag,
    GiveFeedback: drop_source_give_feedback,
};

#[cfg(windows)]
unsafe extern "system" fn enum_format_etc_query_interface(
    this: *mut c_void,
    riid: *const GUID,
    ppv_object: *mut *mut c_void,
) -> HRESULT {
    if riid.is_null() || ppv_object.is_null() {
        return E_POINTER;
    }
    let guid = &*riid;
    if *guid == IID_IUNKNOWN || *guid == IID_IENUMFORMATETC {
        *ppv_object = this;
        enum_format_etc_add_ref(this);
        S_OK
    } else {
        *ppv_object = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

#[cfg(windows)]
unsafe extern "system" fn enum_format_etc_add_ref(this: *mut c_void) -> u32 {
    let obj = &*(this as *mut EnumFormatEtc);
    obj.ref_count.fetch_add(1, Ordering::SeqCst) + 1
}

#[cfg(windows)]
unsafe extern "system" fn enum_format_etc_release(this: *mut c_void) -> u32 {
    let obj = &*(this as *mut EnumFormatEtc);
    let count = obj.ref_count.fetch_sub(1, Ordering::SeqCst) - 1;
    if count == 0 {
        let _ = Box::from_raw(this as *mut EnumFormatEtc);
    }
    count
}

#[cfg(windows)]
unsafe extern "system" fn enum_format_etc_next(
    this: *mut c_void,
    celt: u32,
    rgelt: *mut FORMATETC,
    pcelt_fetched: *mut u32,
) -> HRESULT {
    if rgelt.is_null() || (celt > 1 && pcelt_fetched.is_null()) {
        return E_INVALIDARG;
    }
    let obj = &mut *(this as *mut EnumFormatEtc);
    let mut fetched = 0u32;
    while obj.index < obj.formats.len() && fetched < celt {
        *rgelt.add(fetched as usize) = obj.formats[obj.index];
        obj.index += 1;
        fetched += 1;
    }
    if !pcelt_fetched.is_null() {
        *pcelt_fetched = fetched;
    }
    if fetched == celt {
        S_OK
    } else {
        S_FALSE
    }
}

#[cfg(windows)]
unsafe extern "system" fn enum_format_etc_skip(this: *mut c_void, celt: u32) -> HRESULT {
    let obj = &mut *(this as *mut EnumFormatEtc);
    obj.index = (obj.index + celt as usize).min(obj.formats.len());
    if obj.index < obj.formats.len() {
        S_OK
    } else {
        S_FALSE
    }
}

#[cfg(windows)]
unsafe extern "system" fn enum_format_etc_reset(this: *mut c_void) -> HRESULT {
    let obj = &mut *(this as *mut EnumFormatEtc);
    obj.index = 0;
    S_OK
}

#[cfg(windows)]
unsafe extern "system" fn enum_format_etc_clone(
    this: *mut c_void,
    pp_enum: *mut *mut c_void,
) -> HRESULT {
    if pp_enum.is_null() {
        return E_POINTER;
    }
    let obj = &*(this as *mut EnumFormatEtc);
    let cloned = Box::into_raw(Box::new(EnumFormatEtc {
        vtbl: &ENUM_FORMAT_ETC_VTBL,
        ref_count: AtomicU32::new(1),
        index: obj.index,
        formats: obj.formats.clone(),
    }));
    *pp_enum = cloned as *mut c_void;
    S_OK
}

#[cfg(windows)]
static ENUM_FORMAT_ETC_VTBL: IEnumFORMATETCVtbl = IEnumFORMATETCVtbl {
    QueryInterface: enum_format_etc_query_interface,
    AddRef: enum_format_etc_add_ref,
    Release: enum_format_etc_release,
    Next: enum_format_etc_next,
    Skip: enum_format_etc_skip,
    Reset: enum_format_etc_reset,
    Clone: enum_format_etc_clone,
};

#[cfg(windows)]
pub fn create_hdrop_buffer(paths: &[PathBuf]) -> Result<*mut c_void, CommandError> {
    if paths.is_empty() {
        return Err(CommandError::new(
            "invalid_source",
            "No file paths provided for drag.",
        ));
    }

    let mut wide_chars: Vec<u16> = Vec::new();
    for path in paths {
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide_chars.extend_from_slice(&wide);
        wide_chars.push(0);
    }
    wide_chars.push(0);

    let dropfiles_size = std::mem::size_of::<DROPFILES>();
    let total_bytes = dropfiles_size + wide_chars.len() * std::mem::size_of::<u16>();

    let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total_bytes) };
    if hglobal.is_null() {
        return Err(CommandError::new(
            "out_of_memory",
            "Failed to allocate memory for drag-and-drop.",
        ));
    }

    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        unsafe { GlobalFree(hglobal) };
        return Err(CommandError::new(
            "out_of_memory",
            "Failed to lock memory for drag-and-drop.",
        ));
    }

    unsafe {
        let dropfiles = ptr as *mut DROPFILES;
        (*dropfiles).pFiles = dropfiles_size as u32;
        (*dropfiles).pt = POINT { x: 0, y: 0 };
        (*dropfiles).fNC = 0;
        (*dropfiles).fWide = 1;

        let dest_chars = (ptr as usize + dropfiles_size) as *mut u16;
        std::ptr::copy_nonoverlapping(wide_chars.as_ptr(), dest_chars, wide_chars.len());
        GlobalUnlock(hglobal);
    }

    Ok(hglobal)
}

/// Filters entry paths down to top-level paths so that if "dir" and "dir/file.txt" are both
/// in the list, only "dir" is retained.
pub fn top_level_paths(paths: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = paths
        .iter()
        .map(|p| p.trim_matches('/').replace('\\', "/"))
        .filter(|p| !p.is_empty())
        .collect();
    normalized.sort_by_key(|p| p.len());

    let mut result = Vec::new();
    for p in &normalized {
        let is_child = result
            .iter()
            .any(|parent: &String| p.starts_with(parent) && p[parent.len()..].starts_with('/'));
        if !is_child {
            result.push(p.clone());
        }
    }
    result
}

static CLEANUP_QUEUE: Mutex<Option<Vec<(PathBuf, Instant)>>> = Mutex::new(None);

pub fn register_temp_dir_for_cleanup(path: PathBuf) {
    if let Ok(mut guard) = CLEANUP_QUEUE.lock() {
        let list = guard.get_or_insert_with(Vec::new);
        list.push((path, Instant::now()));
        let five_mins = Duration::from_secs(300);
        list.retain(|(p, time)| {
            if time.elapsed() > five_mins {
                let _ = fs::remove_dir_all(p);
                false
            } else {
                true
            }
        });
    }
}

/// Cleans any stale archi-dnd-* staging directories in %TEMP% left over from past sessions.
pub fn cleanup_old_drag_temp_dirs() {
    let temp = std::env::temp_dir();
    if let Ok(entries) = fs::read_dir(temp) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("archi-dnd-") {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
fn execute_windows_drag(disk_paths: &[PathBuf]) -> Result<(), CommandError> {
    let hglobal = create_hdrop_buffer(disk_paths)?;

    unsafe {
        let _ = OleInitialize(std::ptr::null_mut());

        let data_obj = Box::into_raw(Box::new(FileDataObject {
            vtbl: &DATA_OBJECT_VTBL,
            ref_count: AtomicU32::new(1),
            hglobal,
        }));

        let drop_source = Box::into_raw(Box::new(DropSource {
            vtbl: &DROP_SOURCE_VTBL,
            ref_count: AtomicU32::new(1),
        }));

        let mut dw_effect = 0u32;
        let ok_effects = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
        let _hr = DoDragDrop(
            data_obj as *mut c_void,
            drop_source as *mut c_void,
            ok_effects,
            &mut dw_effect,
        );

        data_object_release(data_obj as *mut c_void);
        drop_source_release(drop_source as *mut c_void);

        OleUninitialize();
    }

    Ok(())
}

/// Staging extraction and native drag execution.
pub fn perform_drag_out(
    archive_path: &Path,
    selected_paths: &[String],
    password: Option<String>,
) -> Result<(), CommandError> {
    if selected_paths.is_empty() {
        return Err(CommandError::new(
            "invalid_selection",
            "No entries selected to drag.",
        ));
    }
    if !archive_path.is_file() {
        return Err(CommandError::new(
            "not_found",
            "Archive file not found or is not a file.",
        ));
    }

    let (temp_dir, disk_paths) = stage_drag_files(archive_path, selected_paths, password)?;

    #[cfg(windows)]
    {
        let res = execute_windows_drag(&disk_paths);
        register_temp_dir_for_cleanup(temp_dir);
        res
    }

    #[cfg(not(windows))]
    {
        let _ = fs::remove_dir_all(&temp_dir);
        Err(CommandError::new(
            "unsupported_platform",
            "Drag-out is only supported on Windows.",
        ))
    }
}

/// Extracts selected archive entries into a staging temporary directory.
/// Returns (temp_dir, disk_paths) on success.
pub fn stage_drag_files(
    archive_path: &Path,
    selected_paths: &[String],
    password: Option<String>,
) -> Result<(PathBuf, Vec<PathBuf>), CommandError> {
    if selected_paths.is_empty() {
        return Err(CommandError::new(
            "invalid_selection",
            "No entries selected to drag.",
        ));
    }
    if !archive_path.is_file() {
        return Err(CommandError::new(
            "not_found",
            "Archive file not found or is not a file.",
        ));
    }

    let top_paths = top_level_paths(selected_paths);
    if top_paths.is_empty() {
        return Err(CommandError::new(
            "invalid_selection",
            "No valid entries to drag.",
        ));
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let rand_suffix: u32 = (stamp & 0xFFFF_FFFF) as u32;
    let temp_folder_name = format!("archi-dnd-{}-{}-{}", std::process::id(), stamp, rand_suffix);
    let temp_dir = std::env::temp_dir().join(temp_folder_name);

    fs::create_dir_all(&temp_dir).map_err(|e| {
        CommandError::new(
            "temp_create_failed",
            format!("Cannot create drag staging directory: {e}"),
        )
    })?;

    let op_id = format!("drag-out-{}-{}", std::process::id(), stamp);
    let cancelled = AtomicBool::new(false);

    let extract_result = extract_any(
        archive_path,
        &temp_dir,
        &op_id,
        &cancelled,
        Some(&top_paths),
        password,
        &FailOnConflict,
        |_| {},
    );

    if let Err(err) = extract_result {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(err);
    }

    let mut disk_paths: Vec<PathBuf> = Vec::new();
    for rel in &top_paths {
        let disk_path = temp_dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if disk_path.exists() {
            disk_paths.push(disk_path);
        }
    }

    if disk_paths.is_empty() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(CommandError::new(
            "extraction_empty",
            "Extracted files could not be found for drag.",
        ));
    }

    Ok((temp_dir, disk_paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_top_level_paths_filtering() {
        let input = vec![
            "folder".to_string(),
            "folder/sub/file.txt".to_string(),
            "folder/file2.txt".to_string(),
            "other.txt".to_string(),
        ];
        let result = top_level_paths(&input);
        assert_eq!(result, vec!["folder".to_string(), "other.txt".to_string()]);
    }

    #[test]
    fn test_top_level_paths_no_overlap() {
        let input = vec![
            "a.txt".to_string(),
            "b/c.txt".to_string(),
            "d/e/f.txt".to_string(),
        ];
        let result = top_level_paths(&input);
        assert_eq!(result.len(), 3);
    }

    #[cfg(windows)]
    #[test]
    fn test_create_hdrop_buffer_structure() {
        let p1 = PathBuf::from(r"C:\test\file1.txt");
        let p2 = PathBuf::from(r"C:\test\file2.txt");
        let hglobal = create_hdrop_buffer(&[p1.clone(), p2.clone()]).expect("create buffer");
        assert!(!hglobal.is_null());

        unsafe {
            let size = GlobalSize(hglobal);
            assert!(size > std::mem::size_of::<DROPFILES>());

            let ptr = GlobalLock(hglobal);
            assert!(!ptr.is_null());
            let dropfiles = ptr as *const DROPFILES;
            assert_eq!((*dropfiles).pFiles, std::mem::size_of::<DROPFILES>() as u32);
            assert_eq!((*dropfiles).fWide, 1);

            let chars_ptr = (ptr as usize + std::mem::size_of::<DROPFILES>()) as *const u16;
            let total_u16 = (size - std::mem::size_of::<DROPFILES>()) / 2;
            let slice = std::slice::from_raw_parts(chars_ptr, total_u16);

            let s1_wide: Vec<u16> = p1.as_os_str().encode_wide().collect();
            let s2_wide: Vec<u16> = p2.as_os_str().encode_wide().collect();

            assert_eq!(&slice[..s1_wide.len()], &s1_wide[..]);
            assert_eq!(slice[s1_wide.len()], 0);

            let offset2 = s1_wide.len() + 1;
            assert_eq!(&slice[offset2..offset2 + s2_wide.len()], &s2_wide[..]);
            assert_eq!(slice[offset2 + s2_wide.len()], 0);
            assert_eq!(slice[offset2 + s2_wide.len() + 1], 0);

            GlobalUnlock(hglobal);
            GlobalFree(hglobal);
        }
    }
}
