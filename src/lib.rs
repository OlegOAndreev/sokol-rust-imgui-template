mod abort_hook;
mod app;
mod imgui_sokol;
mod sg_util;
mod shaders;

pub use app::new_sapp_desc;
// Re-export sokol::app::Desc for android
pub use sokol::app::Desc as SappDesc;
