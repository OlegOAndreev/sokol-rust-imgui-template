use crate::imgui_sokol::ImguiSokol;
use crate::shaders::triangle_shader;
use sokol::{app as sapp, audio as saudio, gfx as sg, glue as sglue, time as stm};
use std::ffi;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

mod abort_hook;
mod imgui_sokol;
mod sg_util;
mod shaders;

// AudioState gets access in a separate thread, so we need to make it thread-safe. Rust rules require us to have a
// separate immutable control object which is shared between two States.
struct AudioControl {
    next_pitch: [AtomicU32; 3],
    next_amplitude: [AtomicU32; 3],
}

impl AudioControl {
    fn set_sine(&self, idx: usize, pitch: f32, amplitude: f32) {
        self.next_pitch[idx].store(pitch.to_bits(), Ordering::Relaxed);
        self.next_amplitude[idx].store(amplitude.to_bits(), Ordering::Relaxed);
    }

    fn get_sine(&self, idx: usize) -> (f32, f32) {
        let pitch = f32::from_bits(self.next_pitch[idx].load(Ordering::Relaxed));
        let amplitude = f32::from_bits(self.next_amplitude[idx].load(Ordering::Relaxed));
        (pitch, amplitude)
    }
}

struct AudioState {
    // Number of audio frames written since the start of playback.
    num_frames: usize,
    cur_pitch: [f32; 3],
    cur_amplitude: [f32; 3],
    control: Arc<AudioControl>,
}

extern "C" fn audio_stream_cb(
    buffer: *mut f32,
    num_frames: ffi::c_int,
    num_channels: ffi::c_int,
    user_data: *mut ffi::c_void,
) {
    abort_hook::ffi_guard("audio_callback", || {
        let buffer = unsafe {
            std::slice::from_raw_parts_mut(buffer, num_channels as usize * num_frames as usize)
        };
        let state = unsafe { (user_data as *mut AudioState).as_mut().unwrap() };
        state.stream(buffer, num_frames as usize, num_channels as usize);
    });
}

impl AudioState {
    fn stream(self: &mut Self, buffer: &mut [f32], num_frames: usize, num_channels: usize) {
        // Play 3 sine waves, interpolating between current and desired state if needed. Buffer size is large enough for
        // CROSSFADE_MS to be shorter than one buffer for typical sample rates.
        const CROSSFADE_MS: usize = 10;
        let crossfade_frames =
            (CROSSFADE_MS * saudio::sample_rate() as usize / 1000).min(num_frames);

        // Forgive me for using f64 crutch here.
        let dt = std::f64::consts::PI * 2.0 / saudio::sample_rate() as f64;

        let (pitch0, amplitude0) = self.control.get_sine(0);
        let (pitch1, amplitude1) = self.control.get_sine(1);
        let (pitch2, amplitude2) = self.control.get_sine(2);
        let (cur_pitch1, cur_amplitude1) = (self.cur_pitch[1], self.cur_amplitude[1]);
        let (cur_pitch2, cur_amplitude2) = (self.cur_pitch[2], self.cur_amplitude[2]);
        let (cur_pitch0, cur_amplitude0) = (self.cur_pitch[0], self.cur_amplitude[0]);
        // Skip crossfading if the parameters have not changed.
        let crossfade_needed = pitch0 != cur_pitch0
            || amplitude0 != cur_amplitude0
            || pitch1 != cur_pitch1
            || amplitude1 != cur_amplitude1
            || pitch2 != cur_pitch2
            || amplitude2 != cur_amplitude2;

        let base_frame = self.num_frames;
        for i in 0..num_frames {
            let pos = (base_frame + i) as f64 * dt;
            let next_sample = (pos * pitch0 as f64).sin() as f32 * amplitude0
                + (pos * pitch1 as f64).sin() as f32 * amplitude1
                + (pos * pitch2 as f64).sin() as f32 * amplitude2;

            let sample = if crossfade_needed && i < crossfade_frames {
                let cur_sample = (pos * cur_pitch0 as f64).sin() as f32 * cur_amplitude0
                    + (pos * cur_pitch1 as f64).sin() as f32 * cur_amplitude1
                    + (pos * cur_pitch2 as f64).sin() as f32 * cur_amplitude2;
                let alpha = i as f32 / crossfade_frames as f32;
                next_sample * alpha + cur_sample * (1.0 - alpha)
            } else {
                next_sample
            };
            for j in 0..num_channels {
                // Normalize the result to avoid clipping
                buffer[i * num_channels + j] = sample * 0.25;
            }
        }
        self.num_frames += num_frames;
        self.cur_pitch[0] = pitch0;
        self.cur_amplitude[0] = amplitude0;
        self.cur_pitch[1] = pitch1;
        self.cur_amplitude[1] = amplitude1;
        self.cur_pitch[2] = pitch2;
        self.cur_amplitude[2] = amplitude2;
    }
}

struct State {
    pass_action: sg::PassAction,
    tri_shader: sg::Shader,
    tri_pipeline: sg::Pipeline,
    tri_bind: sg::Bindings,
    tri_vertex_buf: sg::Buffer,
    imgui: ImguiSokol,
    imgui_sample_text: String,
    color0: [f32; 3],
    color1: [f32; 3],
    color2: [f32; 3],
    last_frame_time: u64,
    fps: f64,
    frame_count: u64,
    fps_update_timer: f64,
    audio_started: bool,
    audio_enabled: bool,
    audio_control: Arc<AudioControl>,
}

extern "C" fn init_cb(user_data: *mut ffi::c_void) {
    abort_hook::ffi_guard("init_cb", || {
        let state = unsafe { (user_data as *mut State).as_mut().unwrap() };
        state.init();
    });
}

extern "C" fn event_cb(event: *const sapp::Event, user_data: *mut ffi::c_void) {
    abort_hook::ffi_guard("event_cb", || {
        let event = unsafe { event.as_ref().unwrap() };
        let state = unsafe { (user_data as *mut State).as_mut().unwrap() };
        state.on_event(event);
    });
}

extern "C" fn frame_cb(user_data: *mut ffi::c_void) {
    abort_hook::ffi_guard("frame_cb", || {
        let state = unsafe { (user_data as *mut State).as_mut().unwrap() };
        state.frame();
    });
}

extern "C" fn cleanup_cb(user_data: *mut ffi::c_void) {
    abort_hook::ffi_guard("cleanup_cb", || {
        let state = unsafe { (user_data as *mut State).as_mut().unwrap() };
        state.cleanup();
    });
}

impl State {
    fn init(self: &mut Self) {
        sg::setup(&sg::Desc {
            environment: sglue::environment(),
            logger: sg::Logger {
                func: Some(sokol::log::slog_func),
                ..Default::default()
            },
            ..Default::default()
        });

        self.tri_vertex_buf = sg::make_buffer(&sg::BufferDesc {
            size: 1,
            usage: sg::BufferUsage {
                vertex_buffer: true,
                write_transient: true,
                ..Default::default()
            },
            ..Default::default()
        });
        self.tri_shader =
            sg::make_shader(&triangle_shader::triangle_shader_desc(sg::query_backend()));

        let mut tri_layout = sg::VertexLayoutState::new();
        tri_layout.attrs[triangle_shader::ATTR_TRIANGLE_POSITION] = sg::VertexAttrState {
            buffer_index: 0,
            offset: 0,
            format: sg::VertexFormat::Float3,
        };
        tri_layout.attrs[triangle_shader::ATTR_TRIANGLE_COLOR0] = sg::VertexAttrState {
            buffer_index: 0,
            offset: 12,
            format: sg::VertexFormat::Float3,
        };
        self.tri_pipeline = sg::make_pipeline(&sg::PipelineDesc {
            shader: self.tri_shader,
            layout: tri_layout,
            primitive_type: sg::PrimitiveType::Triangles,
            color_count: 1,
            ..Default::default()
        });

        self.tri_bind = sg::Bindings::new();

        self.imgui.setup(&crate::imgui_sokol::ImguiSetupDesc {
            font_density: sapp::dpi_scale(),
            font_ttf_data: Some((include_bytes!("../data/fonts/DroidSans.ttf"), 16.0)),
            ..Default::default()
        });

        self.pass_action.colors[0] = sg::ColorAttachmentAction {
            load_action: sg::LoadAction::Clear,
            clear_value: sg::Color {
                r: 0.1,
                g: 0.1,
                b: 0.15,
                a: 1.0,
            },
            ..Default::default()
        };
    }

    // This is used to support web builds: web audio can only be initialized when the user does an action, delay
    // initializing saudio.
    fn setup_audio(self: &mut Self) {
        if self.audio_started {
            return;
        }
        self.audio_started = true;
        let audio_state = Box::new(AudioState {
            num_frames: 0,
            cur_pitch: [0.0; 3],
            cur_amplitude: [0.0; 3],
            control: self.audio_control.clone(),
        });
        let user_data = Box::into_raw(audio_state);

        saudio::setup(&saudio::Desc {
            sample_rate: 48000,
            num_channels: 2,
            buffer_frames: 1024,
            stream_userdata_cb: Some(audio_stream_cb),
            user_data: user_data.cast(),
            ..Default::default()
        });
    }

    fn on_event(self: &mut Self, event: &sapp::Event) {
        #[cfg(target_os = "macos")]
        const QUIT_MODIFIER: u32 = sapp::MODIFIER_SUPER;
        #[cfg(not(target_os = "macos"))]
        const QUIT_MODIFIER: u32 = sapp::MODIFIER_CTRL;

        // See start_audio for why this is needed on the web.
        if event._type == sapp::EventType::KeyDown
            || event._type == sapp::EventType::MouseDown
            || event._type == sapp::EventType::TouchesBegan
        {
            self.setup_audio();
        }

        if event._type == sapp::EventType::KeyDown {
            if event.key_code == sapp::Keycode::Q && event.modifiers & QUIT_MODIFIER != 0 {
                sapp::quit();
                return;
            } else if event.key_code == sapp::Keycode::F11 {
                sapp::toggle_fullscreen();
            }
        }

        self.imgui.handle_event(event);
    }

    fn frame(self: &mut Self) {
        let dt = self.update_time();
        self.ui_frame(dt);

        self.render();
        sg::commit();
    }

    fn update_time(self: &mut Self) -> f64 {
        let now = stm::now();
        let dt = stm::sec(now - self.last_frame_time);
        self.last_frame_time = now;
        self.frame_count += 1;
        self.fps_update_timer += dt;
        if self.fps_update_timer >= 0.5 {
            self.fps = self.frame_count as f64 / self.fps_update_timer;
            self.frame_count = 0;
            self.fps_update_timer = 0.0;
        }
        dt
    }

    fn render(self: &mut Self) {
        #[rustfmt::skip]
        let tri_verts: [f32; _] = [
            -0.5, -0.5, 0.0, self.color0[0], self.color0[1], self.color0[2],
             0.5, -0.5, 0.0, self.color1[0], self.color1[1], self.color1[2],
             0.0,  0.5, 0.0, self.color2[0], self.color2[1], self.color2[2],
        ];
        sg_util::update_transient_buffer(&mut self.tri_vertex_buf, &tri_verts);

        self.imgui.prepare_data();

        sg::begin_pass(&sg::Pass {
            action: self.pass_action,
            swapchain: sglue::swapchain(),
            ..Default::default()
        });

        sg::apply_pipeline(self.tri_pipeline);
        self.tri_bind.vertex_buffers[0] = self.tri_vertex_buf;
        sg::apply_bindings(&self.tri_bind);
        sg::draw(0, 3, 1);

        self.imgui.render();

        sg::end_pass();
    }

    fn ui_frame(self: &mut Self, dt: f64) {
        let ui = self
            .imgui
            .new_frame(sapp::width(), sapp::height(), dt, sapp::dpi_scale());

        ui.window("Hello, imgui!")
            .position([400.0, 20.0], imgui::Condition::FirstUseEver)
            .size([360.0, 520.0], imgui::Condition::FirstUseEver)
            .build(|| {
                ui.text(format!("Framebuffer: {}x{}", sapp::width(), sapp::height()));
                ui.text(format!("FPS: {:.1}", self.fps));
                ui.text("DPI scale: ");
                ui.same_line();
                ui.text(format!("{:.2}", sapp::dpi_scale()));
                let fullscreen_title = if sapp::is_fullscreen() {
                    "Toggle Windowed"
                } else {
                    "Toggle Fullscreen"
                };
                if ui.button(fullscreen_title) {
                    sapp::toggle_fullscreen();
                }

                ui.spacing();
                ui.separator();
                ui.spacing();

                ui.text("Text input:");
                ui.input_text("##imgui_text", &mut self.imgui_sample_text)
                    .build();

                ui.separator();
                ui.text("Vertex colors:");
                ui.color_edit3("V0", &mut self.color0);
                ui.color_edit3("V1", &mut self.color1);
                ui.color_edit3("V2", &mut self.color2);

                ui.spacing();
                ui.separator();
                ui.spacing();

                ui.checkbox("Play audio", &mut self.audio_enabled);
                if self.audio_enabled {
                    let (pitch0, amplitude0) = color_to_pitch_and_amplitude(&self.color0);
                    self.audio_control.set_sine(0, pitch0, amplitude0);
                    let (pitch1, amplitude1) = color_to_pitch_and_amplitude(&self.color1);
                    self.audio_control.set_sine(1, pitch1, amplitude1);
                    let (pitch2, amplitude2) = color_to_pitch_and_amplitude(&self.color2);
                    self.audio_control.set_sine(2, pitch2, amplitude2);
                } else {
                    self.audio_control.set_sine(0, 0.0, 0.0);
                    self.audio_control.set_sine(1, 0.0, 0.0);
                    self.audio_control.set_sine(2, 0.0, 0.0);
                }

                ui.spacing();
                ui.separator();
                ui.spacing();

                ui.text("Scroll test:");
                ui.child_window("##scroll").size([0.0, 0.0]).build(|| {
                    for i in 0..200 {
                        ui.text(format!("This is row {}/200", i + 1));
                    }
                });
            });
    }

    fn cleanup(self: &mut Self) {
        self.imgui.shutdown();
        sg::destroy_pipeline(self.tri_pipeline);
        self.tri_pipeline = sg::Pipeline::new();
        sg::destroy_shader(self.tri_shader);
        self.tri_shader = sg::Shader::new();
        sg::destroy_buffer(self.tri_vertex_buf);
        self.tri_vertex_buf = sg::Buffer::new();
        sg::shutdown();
    }
}

// Choose the pitch by mapping color hue to a pitch range, amplitude equals color saturation.
fn color_to_pitch_and_amplitude(color: &[f32; 3]) -> (f32, f32) {
    const MIN_PITCH: f32 = 200.0;
    const MAX_PITCH: f32 = 1000.0;

    let (r, g, b) = (color[0], color[1], color[2]);
    let min = r.min(g).min(b);
    let max = r.max(g).max(b);
    let diff = max - min;
    if diff < 1e-5 {
        return (0.0, 0.0);
    }
    let hue = if max == r {
        if g >= b {
            (g - b) / (diff * 6.0)
        } else {
            (g - b) / (diff * 6.0) + 1.0
        }
    } else if max == g {
        (b - r) / (diff * 6.0) + 1.0 / 3.0
    } else {
        (r - g) / (diff * 6.0) + 2.0 / 3.0
    };

    let pitch = MIN_PITCH + (MAX_PITCH - MIN_PITCH) * hue;
    let amplitude = 1.0 - min / max;

    (pitch, amplitude)
}

// Used for sokol_main on Android and main on other OS
pub fn new_sapp_desc() -> sapp::Desc {
    println!("Test println");
    std::panic::set_hook(Box::new(|info| {
        eprintln!("=== Rust panic ===");
        eprintln!("{info}");
    }));

    stm::setup();

    let audio_control = Arc::new(AudioControl {
        next_pitch: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
        next_amplitude: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
    });
    let state = Box::new(State {
        pass_action: sg::PassAction::new(),
        tri_shader: sg::Shader::new(),
        tri_pipeline: sg::Pipeline::new(),
        tri_bind: sg::Bindings::new(),
        tri_vertex_buf: sg::Buffer::new(),
        imgui: ImguiSokol::new(),
        color0: [1.0, 0.0, 0.0],
        color1: [0.0, 1.0, 0.0],
        color2: [0.0, 0.0, 1.0],
        last_frame_time: stm::now(),
        fps: 0.0,
        frame_count: 0,
        fps_update_timer: 0.0,
        imgui_sample_text: "type here...".into(),
        audio_started: false,
        audio_enabled: false,
        audio_control,
    });
    let user_data = Box::into_raw(state);

    sapp::Desc {
        init_userdata_cb: Some(init_cb),
        frame_userdata_cb: Some(frame_cb),
        cleanup_userdata_cb: Some(cleanup_cb),
        event_userdata_cb: Some(event_cb),
        user_data: user_data.cast(),
        window_title: c"sokol-rust example".as_ptr(),
        enable_clipboard: true,
        width: 800,
        height: 600,
        sample_count: 1,
        logger: sapp::Logger {
            func: Some(sokol::log::slog_func),
            ..Default::default()
        },
        icon: sapp::IconDesc {
            sokol_default: true,
            ..Default::default()
        },
        high_dpi: true,
        ..Default::default()
    }
}

fn main() {
    sapp::run(&new_sapp_desc());
}
