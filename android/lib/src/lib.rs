use std::ffi;

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn sokol_main(
    _argc: ffi::c_int,
    _argv: *mut *mut ffi::c_char,
) -> sokol_rust_imgui_template::SappDesc {
    sokol_rust_imgui_template::new_sapp_desc()
}
