use sokol::app as sapp;

#[cfg(not(target_os = "android"))]
fn main() {
    sapp::run(&sokol_rust_imgui_template::new_sapp_desc());
}
