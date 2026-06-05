#![allow(dead_code)]

/// Dear ImGui renderer and event-handler for sokol. This is a Rust port of
/// https://github.com/floooh/sokol/blob/master/util/sokol_imgui.h
use sokol::{app as sapp, gfx as sg};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::{sg_util, shaders::imgui_shader};

// In emscripten we need to dynamically check if the user is running on macos or other OS for shortcuts (Ctrl+C/Cmd+C
// etc). See
// https://developer.mozilla.org/en-US/docs/Web/API/Navigator/platform#determining_the_modifier_key_for_the_users_platform
fn ui_is_macos() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        #[cfg(not(target_os = "emscripten"))]
        {
            cfg!(any(target_os = "macos", target_os = "ios"))
        }
        #[cfg(target_os = "emscripten")]
        {
            unsafe extern "C-unwind" {
                fn emscripten_run_script_int(script: *const std::ffi::c_char) -> std::ffi::c_int;
            }

            println!("Detecting emscripten macos...");
            let result = unsafe {
                emscripten_run_script_int(
                    c"navigator.platform.startsWith('Mac') || navigator.platform === 'iPhone' ? 1 : 0".as_ptr(),
                ) != 0
            };
            println!("Detected emscripten macos: {}", result);
            result
        }
    })
}

struct SappClipboardBackend {}

impl imgui::ClipboardBackend for SappClipboardBackend {
    fn get(&mut self) -> Option<String> {
        let contents = sapp::get_clipboard_string();
        if contents.is_empty() {
            return None;
        }
        Some(contents.to_owned())
    }

    fn set(&mut self, value: &str) {
        sapp::set_clipboard_string(value);
    }
}

#[derive(Clone)]
pub struct ImguiSetupDesc {
    /// Render font with higher quality, usually should be set to dpi_scale.
    pub font_density: f32,
    /// Optional TTF font data to use instead of the default font. When set, the font is loaded at the given size (in
    /// pixels) before building the atlas.
    pub font_ttf_data: Option<(&'static [u8], f32)>,
    /// Path where ImGui stores/loads UI persistency data (None = disabled)
    pub ini_filename: Option<String>,
    /// If true, don't forward Ctrl-V paste events
    pub disable_paste_override: bool,
    /// If true, alpha values get written into the framebuffer
    pub write_alpha_channel: bool,
}

impl Default for ImguiSetupDesc {
    fn default() -> Self {
        Self {
            font_density: 1.0,
            font_ttf_data: None,
            ini_filename: None,
            disable_paste_override: false,
            write_alpha_channel: false,
        }
    }
}

pub struct ImguiSokol {
    context: imgui::Context,
    desc: ImguiSetupDesc,
    shader: sg::Shader,
    pipeline: sg::Pipeline,
    vertex_buf: sg::Buffer,
    index_buf: sg::Buffer,
    default_sampler: sg::Sampler,
    vertices: Vec<imgui::DrawVert>,
    indices: Vec<imgui::DrawIdx>,
    cur_dpi_scale: f32,
    font_texture: sg::Image,
    font_view: sg::View,
    font_sampler: sg::Sampler,
    textures: imgui::Textures<(sg::View, sg::Sampler)>,
}

impl ImguiSokol {
    /// Create a new (uninitialized) ImguiSokol instance. Call setup() before all other methods.
    pub fn new() -> Self {
        let context = imgui::Context::create();
        Self {
            context,
            desc: ImguiSetupDesc::default(),
            shader: sg::Shader::new(),
            pipeline: sg::Pipeline::new(),
            vertex_buf: sg::Buffer::new(),
            index_buf: sg::Buffer::new(),
            default_sampler: sg::Sampler::new(),
            vertices: vec![],
            indices: vec![],
            cur_dpi_scale: 1.0,
            font_texture: sg::Image::new(),
            font_view: sg::View::new(),
            font_sampler: sg::Sampler::new(),
            textures: imgui::Textures::new(),
        }
    }

    /// Create all required sg resources, must be called once after sg::setup().
    pub fn setup(&mut self, desc: &ImguiSetupDesc) {
        self.desc = desc.clone();

        self.context
            .set_renderer_name(Some("sokol-imgui".to_string()));

        self.context
            .set_ini_filename(self.desc.ini_filename.as_ref().map(PathBuf::from));

        {
            let io = self.context.io_mut();
            io.display_size = [1.0, 1.0];
            io.config_mac_os_behaviors = ui_is_macos();
            io.config_windows_resize_from_edges = true;
            io.config_input_text_cursor_blink = false;
            io.backend_flags
                .insert(imgui::BackendFlags::RENDERER_HAS_VTX_OFFSET);
            io.backend_flags
                .insert(imgui::BackendFlags::HAS_MOUSE_CURSORS);
        }

        let write_mask = if self.desc.write_alpha_channel {
            sg::ColorMask::Rgba
        } else {
            sg::ColorMask::Rgb
        };

        self.shader = sg::make_shader(&imgui_shader::imgui_shader_desc(sg::query_backend()));
        let mut layout = sg::VertexLayoutState::new();
        layout.attrs[imgui_shader::ATTR_IMGUI_POSITION] = sg::VertexAttrState {
            buffer_index: 0,
            offset: std::mem::offset_of!(imgui::sys::ImDrawVert, pos) as i32,
            format: sg::VertexFormat::Float2,
        };
        layout.attrs[imgui_shader::ATTR_IMGUI_TEXCOORD] = sg::VertexAttrState {
            buffer_index: 0,
            offset: std::mem::offset_of!(imgui::sys::ImDrawVert, uv) as i32,
            format: sg::VertexFormat::Float2,
        };
        layout.attrs[imgui_shader::ATTR_IMGUI_COLOR] = sg::VertexAttrState {
            buffer_index: 0,
            offset: std::mem::offset_of!(imgui::sys::ImDrawVert, col) as i32,
            format: sg::VertexFormat::Ubyte4n,
        };

        let mut pipeline_desc = sg::PipelineDesc {
            shader: self.shader,
            layout,
            index_type: sg::IndexType::Uint16,
            primitive_type: sg::PrimitiveType::Triangles,
            color_count: 1,
            label: c"imgui-sokol-pipeline".as_ptr(),
            ..Default::default()
        };
        pipeline_desc.colors[0] = sg::ColorTargetState {
            write_mask,
            blend: sg::BlendState {
                enabled: true,
                src_factor_rgb: sg::BlendFactor::SrcAlpha,
                dst_factor_rgb: sg::BlendFactor::OneMinusSrcAlpha,
                op_rgb: sg::BlendOp::Add,
                src_factor_alpha: if self.desc.write_alpha_channel {
                    sg::BlendFactor::One
                } else {
                    sg::BlendFactor::SrcAlpha
                },
                dst_factor_alpha: if self.desc.write_alpha_channel {
                    sg::BlendFactor::One
                } else {
                    sg::BlendFactor::OneMinusSrcAlpha
                },
                op_alpha: sg::BlendOp::Add,
            },
            ..Default::default()
        };
        self.pipeline = sg::make_pipeline(&pipeline_desc);

        self.default_sampler = sg::make_sampler(&sg::SamplerDesc {
            min_filter: sg::Filter::Nearest,
            mag_filter: sg::Filter::Nearest,
            wrap_u: sg::Wrap::ClampToEdge,
            wrap_v: sg::Wrap::ClampToEdge,
            label: c"imgui-sokol-default-sampler".as_ptr(),
            ..Default::default()
        });

        self.rebuild_font_texture();

        self.context_mut()
            .set_clipboard_backend(SappClipboardBackend {});
    }

    /// Shut down ImGui and release all sg resources.
    pub fn shutdown(&mut self) {
        sg::destroy_sampler(self.font_sampler);
        self.font_sampler = sg::Sampler::new();
        sg::destroy_view(self.font_view);
        self.font_view = sg::View::new();
        sg::destroy_image(self.font_texture);
        self.font_texture = sg::Image::new();

        sg::destroy_sampler(self.default_sampler);
        self.default_sampler = sg::Sampler::new();
        sg::destroy_pipeline(self.pipeline);
        self.pipeline = sg::Pipeline::new();
        sg::destroy_shader(self.shader);
        self.shader = sg::Shader::new();
        sg::destroy_buffer(self.index_buf);
        self.index_buf = sg::Buffer::new();
        sg::destroy_buffer(self.vertex_buf);
        self.vertex_buf = sg::Buffer::new();

        self.vertices.clear();
        self.indices.clear();
    }

    pub fn context(&self) -> &imgui::Context {
        &self.context
    }

    pub fn context_mut(&mut self) -> &mut imgui::Context {
        &mut self.context
    }

    /// Start a new ImGui frame.
    pub fn new_frame(&mut self, width: i32, height: i32, delta_time: f64, dpi_scale: f32) {
        assert!(width > 0 && height > 0);

        self.cur_dpi_scale = if dpi_scale > 0.0 { dpi_scale } else { 1.0 };

        let io = self.context.io_mut();
        io.display_size = [
            width as f32 / self.cur_dpi_scale,
            height as f32 / self.cur_dpi_scale,
        ];
        io.display_framebuffer_scale = [self.cur_dpi_scale, self.cur_dpi_scale];
        io.delta_time = delta_time as f32;

        if let Some(cursor) = self.context.mouse_cursor() {
            sapp::set_mouse_cursor(map_cursor(cursor));
        }
    }

    /// Renders all ImGui draw data, must be called inside a sg render pass.
    pub fn render(&mut self) {
        let disp_size = self.context.io().display_size;
        let draw_data = self.context.render();
        if draw_data.draw_lists_count() == 0 {
            return;
        }

        sg::push_debug_group("imgui-sokol");

        // Copy all vertex and index data into intermediate buffers and update GPU buffers.
        self.vertices.clear();
        self.indices.clear();
        for draw_list in draw_data.draw_lists() {
            self.vertices.extend_from_slice(draw_list.vtx_buffer());
            self.indices.extend_from_slice(draw_list.idx_buffer());
        }
        sg_util::update_dynamic_buffer(
            &mut self.vertex_buf,
            sg::BufferUsage {
                vertex_buffer: true,
                stream_update: true,
                ..Default::default()
            },
            &self.vertices,
        );
        sg_util::update_dynamic_buffer(
            &mut self.index_buf,
            sg::BufferUsage {
                index_buffer: true,
                stream_update: true,
                ..Default::default()
            },
            &self.indices,
        );

        let fb_scale = draw_data.framebuffer_scale;
        let fb_width = (disp_size[0] * fb_scale[0]) as i32;
        let fb_height = (disp_size[1] * fb_scale[1]) as i32;
        sg::apply_viewport(0, 0, fb_width, fb_height, true);

        sg::apply_pipeline(self.pipeline);
        let vs_params = imgui_shader::VsParams {
            disp_size,
            _pad_8: [0; _],
        };
        sg_util::apply_uniforms(imgui_shader::UB_VS_PARAMS, &vs_params);

        let mut bind = sg::Bindings::new();
        bind.vertex_buffers[0] = self.vertex_buf;
        bind.index_buffer = self.index_buf;

        let mut vb_offset = 0;
        let mut ib_offset = 0;

        for (idx, draw_list) in draw_data.draw_lists().enumerate() {
            bind.vertex_buffer_offsets[0] = vb_offset;
            bind.index_buffer_offset = ib_offset;
            // Do not apply bindings for first element: we will apply them later when switching textures anyway.
            if idx > 0 {
                sg::apply_bindings(&bind);
            }
            // Always apply bindings for the first command in draw_list, because vb_offset and ib_offset got updated.
            let mut prev_tex_id = imgui::TextureId::new(usize::MAX);
            let mut prev_vtx_offset = usize::MAX;

            for cmd in draw_list.commands() {
                match cmd {
                    imgui::DrawCmd::Elements { count, cmd_params } => {
                        if count == 0 {
                            continue;
                        }

                        let tex_id = cmd_params.texture_id;
                        // Reapply bindings if the texture or vtx_offset has changed.
                        if prev_tex_id != tex_id || prev_vtx_offset != cmd_params.vtx_offset {
                            if let Some((view, sampler)) = self.textures.get(tex_id) {
                                bind.views[imgui_shader::VIEW_TEX] = *view;
                                bind.samplers[imgui_shader::SMP_SMP] = *sampler;
                            } else {
                                eprintln!("Unknown texture id {:?} is passed", tex_id);
                                continue;
                            }
                            bind.vertex_buffer_offsets[0] = vb_offset
                                + (cmd_params.vtx_offset * size_of::<imgui::DrawVert>()) as i32;
                            sg::apply_bindings(&bind);
                            prev_tex_id = tex_id;
                            prev_vtx_offset = cmd_params.vtx_offset;
                        }

                        let clip = cmd_params.clip_rect;
                        let scissor_x = (clip[0] * fb_scale[0]) as i32;
                        let scissor_y = (clip[1] * fb_scale[1]) as i32;
                        let scissor_w = ((clip[2] - clip[0]) * fb_scale[0]) as i32;
                        let scissor_h = ((clip[3] - clip[1]) * fb_scale[1]) as i32;
                        sg::apply_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h, true);
                        sg::draw(cmd_params.idx_offset, count, 1);
                    }
                    imgui::DrawCmd::ResetRenderState => {
                        // No-op
                    }
                    imgui::DrawCmd::RawCallback { .. } => {
                        // Callbacks are not supported in this backend
                    }
                }
            }

            vb_offset += size_of_val(draw_list.vtx_buffer()) as i32;
            ib_offset += size_of_val(draw_list.idx_buffer()) as i32;
        }

        // Reset scissor
        sg::apply_scissor_rect(0, 0, fb_width, fb_height, true);
        sg::pop_debug_group();
    }

    /// Handle sapp event, returns true if imgui wants to capture keyboard/mouse input.
    pub fn handle_event(&mut self, event: &sapp::Event) -> bool {
        let dpi_scale = self.cur_dpi_scale;
        let disable_paste = self.desc.disable_paste_override;

        let io = self.context.io_mut();

        match event._type {
            sapp::EventType::Focused => {
                io.app_focus_lost = false;
            }
            sapp::EventType::Unfocused => {
                io.app_focus_lost = true;
            }
            sapp::EventType::MouseEnter | sapp::EventType::MouseLeave => {
                // See comment in https://github.com/floooh/sokol/blob/master/util/sokol_imgui.h for why this is
                // required.
                if cfg!(target_os = "emscripten") {
                    io.add_mouse_button_event(
                        mouse_button_to_imgui(sapp::Mousebutton::Left),
                        false,
                    );
                    io.add_mouse_button_event(
                        mouse_button_to_imgui(sapp::Mousebutton::Middle),
                        false,
                    );
                    io.add_mouse_button_event(
                        mouse_button_to_imgui(sapp::Mousebutton::Right),
                        false,
                    );
                }
            }
            sapp::EventType::MouseDown | sapp::EventType::MouseUp => {
                let down = event._type == sapp::EventType::MouseDown;
                io.add_mouse_pos_event([event.mouse_x / dpi_scale, event.mouse_y / dpi_scale]);
                io.add_mouse_button_event(mouse_button_to_imgui(event.mouse_button), down);
                update_modifiers_io(io, event.modifiers);
            }
            sapp::EventType::MouseMove => {
                io.add_mouse_pos_event([event.mouse_x / dpi_scale, event.mouse_y / dpi_scale]);
            }
            sapp::EventType::MouseScroll => {
                io.add_mouse_wheel_event([event.scroll_x, event.scroll_y]);
            }
            sapp::EventType::TouchesBegan => {
                if event.num_touches > 0 {
                    io.add_mouse_pos_event([
                        event.touches[0].pos_x / dpi_scale,
                        event.touches[0].pos_y / dpi_scale,
                    ]);
                    io.add_mouse_button_event(imgui::MouseButton::Left, true);
                }
            }
            sapp::EventType::TouchesMoved => {
                if event.num_touches > 0 {
                    io.add_mouse_pos_event([
                        event.touches[0].pos_x / dpi_scale,
                        event.touches[0].pos_y / dpi_scale,
                    ]);
                }
            }
            sapp::EventType::TouchesEnded | sapp::EventType::TouchesCancelled => {
                io.add_mouse_button_event(imgui::MouseButton::Left, false);
            }
            sapp::EventType::KeyDown | sapp::EventType::KeyUp => {
                update_modifiers_io(io, event.modifiers);

                let is_ctrl = if ui_is_macos() {
                    event.modifiers & sapp::MODIFIER_SUPER != 0
                } else {
                    event.modifiers & sapp::MODIFIER_CTRL != 0
                };
                if !disable_paste && is_ctrl && event.key_code == sapp::Keycode::V {
                    // Handled by ClipboardPasted event
                } else {
                    // On web platform, don't forward Ctrl-X, Ctrl-V to the browser
                    if is_ctrl
                        && (event.key_code == sapp::Keycode::X
                            || event.key_code == sapp::Keycode::C)
                    {
                        sapp::consume_event();
                    }
                    if let Some(key) = map_keycode(event.key_code) {
                        let down = event._type == sapp::EventType::KeyDown;
                        io.add_key_event(key, down);
                    }
                }
            }
            sapp::EventType::Char => {
                update_modifiers_io(io, event.modifiers);
                if (event.modifiers
                    & (sapp::MODIFIER_ALT | sapp::MODIFIER_CTRL | sapp::MODIFIER_SUPER))
                    == 0
                    && let Some(ch) = char::from_u32(event.char_code)
                {
                    io.add_input_character(ch);
                }
            }
            sapp::EventType::ClipboardPasted if !disable_paste => {
                // Simulate a Ctrl-V key down/up
                let ctrl = if ui_is_macos() {
                    imgui::Key::ModSuper
                } else {
                    imgui::Key::ModCtrl
                };
                io.add_key_event(ctrl, true);
                io.add_key_event(imgui::Key::V, true);
                io.add_key_event(imgui::Key::V, false);
                io.add_key_event(ctrl, false);
            }
            _ => {}
        }

        let io = self.context.io();
        show_keyboard(io.want_text_input);
        io.want_capture_keyboard || io.want_capture_mouse
    }

    /// Allocate new TextureId to use in imgui.
    pub fn create_texture_id(&mut self, view: sg::View, sampler: sg::Sampler) -> imgui::TextureId {
        self.textures.insert((view, sampler))
    }

    fn rebuild_font_texture(&mut self) {
        let mut font_config = imgui::FontConfig {
            // Improve font quality
            rasterizer_density: self.desc.font_density,
            ..Default::default()
        };
        if let Some((ttf_data, size_pixels)) = self.desc.font_ttf_data {
            font_config.glyph_ranges = build_glyph_ranges(ttf_data);
            self.context.fonts().add_font(&[imgui::FontSource::TtfData {
                data: ttf_data,
                size_pixels,
                config: Some(font_config),
            }]);
        } else {
            self.context
                .fonts()
                .add_font(&[imgui::FontSource::DefaultFontData {
                    config: Some(font_config),
                }]);
        }

        let font_image = self.context.fonts().build_rgba32_texture();
        let width = font_image.width as i32;
        let height = font_image.height as i32;

        let mut mip_levels = [sg::Range::new(); 16];
        mip_levels[0] = sg::Range {
            ptr: font_image.data.as_ptr().cast(),
            size: font_image.data.len(),
        };

        self.font_texture = sg::make_image(&sg::ImageDesc {
            width,
            height,
            pixel_format: sg::PixelFormat::Rgba8,
            data: sg::ImageData { mip_levels },
            label: c"imgui-sokol-font-image".as_ptr(),
            ..Default::default()
        });

        self.font_view = sg::make_view(&sg::ViewDesc {
            texture: sg::TextureViewDesc {
                image: self.font_texture,
                ..Default::default()
            },
            label: c"imgui-sokol-font-view".as_ptr(),
            ..Default::default()
        });

        self.font_sampler = sg::make_sampler(&sg::SamplerDesc {
            min_filter: sg::Filter::Linear,
            mag_filter: sg::Filter::Linear,
            wrap_u: sg::Wrap::ClampToEdge,
            wrap_v: sg::Wrap::ClampToEdge,
            label: c"imgui-sokol-font-sampler".as_ptr(),
            ..Default::default()
        });

        self.context.fonts().tex_id = self.create_texture_id(self.font_view, self.font_sampler);
    }
}

fn show_keyboard(want_text_input: bool) {
    if !cfg!(target_os = "ios") {
        return;
    }
    if want_text_input != sapp::keyboard_shown() {
        sapp::show_keyboard(want_text_input);
    }
}

/// Mapping from UnicodeRanges bit indices to (start, end) codepoint pairs. Derived from the `char_range_index` function
/// in ttf-parser. See also https://learn.microsoft.com/en-us/typography/opentype/spec/os2#ur
#[rustfmt::skip]
static UNICODE_RANGE_TABLE: [&[u32]; 123] = [
    // 0. Imgui asserts if the glyph range includes 0x0, start with 0x20
    &[0x0020, 0x007F],
    // 1
    &[0x0080, 0x00FF],
    // 2
    &[0x0100, 0x017F],
    // 3
    &[0x0180, 0x024F],
    // 4
    &[0x0250, 0x02AF, 0x1D00, 0x1DBF],
    // 5
    &[0x02B0, 0x02FF, 0xA700, 0xA71F],
    // 6
    &[0x0300, 0x036F, 0x1DC0, 0x1DFF],
    // 7
    &[0x0370, 0x03FF],
    // 8
    &[0x2C80, 0x2CFF],
    // 9
    &[0x0400, 0x052F, 0x2DE0, 0x2DFF, 0xA640, 0xA69F],
    // 10
    &[0x0530, 0x058F],
    // 11
    &[0x0590, 0x05FF],
    // 12
    &[0xA500, 0xA63F],
    // 13
    &[0x0600, 0x06FF, 0x0750, 0x077F],
    // 14
    &[0x07C0, 0x07FF],
    // 15
    &[0x0900, 0x097F],
    // 16
    &[0x0980, 0x09FF],
    // 17
    &[0x0A00, 0x0A7F],
    // 18
    &[0x0A80, 0x0AFF],
    // 19
    &[0x0B00, 0x0B7F],
    // 20
    &[0x0B80, 0x0BFF],
    // 21
    &[0x0C00, 0x0C7F],
    // 22
    &[0x0C80, 0x0CFF],
    // 23
    &[0x0D00, 0x0D7F],
    // 24
    &[0x0E00, 0x0E7F],
    // 25
    &[0x0E80, 0x0EFF],
    // 26
    &[0x10A0, 0x10FF, 0x2D00, 0x2D2F],
    // 27
    &[0x1B00, 0x1B7F],
    // 28
    &[0x1100, 0x11FF],
    // 29
    &[0x1E00, 0x1EFF, 0x2C60, 0x2C7F, 0xA720, 0xA7FF],
    // 30
    &[0x1F00, 0x1FFF],
    // 31
    &[0x2000, 0x206F, 0x2E00, 0x2E7F],
    // 32
    &[0x2070, 0x209F],
    // 33
    &[0x20A0, 0x20CF],
    // 34
    &[0x20D0, 0x20FF],
    // 35
    &[0x2100, 0x214F],
    // 36
    &[0x2150, 0x218F],
    // 37
    &[0x2190, 0x21FF, 0x27F0, 0x27FF, 0x2900, 0x297F, 0x2B00, 0x2BFF],
    // 38
    &[0x2200, 0x22FF, 0x27C0, 0x27EF, 0x2980, 0x29FF, 0x2A00, 0x2AFF],
    // 39
    &[0x2300, 0x23FF],
    // 40
    &[0x2400, 0x243F],
    // 41
    &[0x2440, 0x245F],
    // 42
    &[0x2460, 0x24FF],
    // 43
    &[0x2500, 0x257F],
    // 44
    &[0x2580, 0x259F],
    // 45
    &[0x25A0, 0x25FF],
    // 46
    &[0x2600, 0x26FF],
    // 47
    &[0x2700, 0x27BF],
    // 48
    &[0x3000, 0x303F],
    // 49
    &[0x3040, 0x309F],
    // 50
    &[0x30A0, 0x30FF, 0x31F0, 0x31FF],
    // 51
    &[0x3100, 0x312F, 0x31A0, 0x31BF],
    // 52
    &[0x3130, 0x318F],
    // 53
    &[0xA840, 0xA87F],
    // 54
    &[0x3200, 0x32FF],
    // 55
    &[0x3300, 0x33FF],
    // 56
    &[0xAC00, 0xD7AF],
    // 57 ignored, "Setting this bit implies there is at least one character beyond the Basic Multilingual Plane
    // supported by this font. First assigned in OpenType 1.3 for OS/2 version 2."
    &[],
    // 58
    &[0x10900, 0x1091F],
    // 59
    &[0x2E80, 0x2FDF, 0x2FF0, 0x2FFF, 0x3190, 0x319F, 0x3400, 0x4DBF, 0x4E00, 0x9FFF, 0x20000, 0x2A6DF],
    // 60
    &[0xE000, 0xF8FF],
    // 61
    &[0x31C0, 0x31EF, 0xF900, 0xFAFF, 0x2F800, 0x2FA1F],
    // 62
    &[0xFB00, 0xFB4F],
    // 63
    &[0xFB50, 0xFDFF],
    // 64
    &[0xFE20, 0xFE2F],
    // 65
    &[0xFE10, 0xFE1F, 0xFE30, 0xFE4F],
    // 66
    &[0xFE50, 0xFE6F],
    // 67
    &[0xFE70, 0xFEFF],
    // 68
    &[0xFF00, 0xFFEF],
    // 69
    &[0xFFF0, 0xFFFF],
    // 70
    &[0x0F00, 0x0FFF],
    // 71
    &[0x0700, 0x074F],
    // 72
    &[0x0780, 0x07BF],
    // 73
    &[0x0D80, 0x0DFF],
    // 74
    &[0x1000, 0x109F],
    // 75
    &[0x1200, 0x139F, 0x2D80, 0x2DDF],
    // 76
    &[0x13A0, 0x13FF],
    // 77
    &[0x1400, 0x167F],
    // 78
    &[0x1680, 0x169F],
    // 79
    &[0x16A0, 0x16FF],
    // 80
    &[0x1780, 0x17FF, 0x19E0, 0x19FF],
    // 81
    &[0x1800, 0x18AF],
    // 82
    &[0x2800, 0x28FF],
    // 83
    &[0xA000, 0xA4CF],
    // 84
    &[0x1700, 0x177F],
    // 85
    &[0x10300, 0x1032F],
    // 86
    &[0x10330, 0x1034F],
    // 87
    &[0x10400, 0x1044F],
    // 88
    &[0x1D000, 0x1D24F],
    // 89
    &[0x1D400, 0x1D7FF],
    // 90
    &[0xF0000, 0xFFFFD, 0x100000, 0x10FFFD],
    // 91
    &[0xFE00, 0xFE0F, 0xE0100, 0xE01EF],
    // 92
    &[0xE0000, 0xE007F],
    // 93
    &[0x1900, 0x194F],
    // 94
    &[0x1950, 0x197F],
    // 95
    &[0x1980, 0x19DF],
    // 96
    &[0x1A00, 0x1A1F],
    // 97
    &[0x2C00, 0x2C5F],
    // 98
    &[0x2D30, 0x2D7F],
    // 99
    &[0x4DC0, 0x4DFF],
    // 100
    &[0xA800, 0xA82F],
    // 101
    &[0x10000, 0x1013F],
    // 102
    &[0x10140, 0x1018F],
    // 103
    &[0x10380, 0x1039F],
    // 104
    &[0x103A0, 0x103DF],
    // 105
    &[0x10450, 0x1047F],
    // 106
    &[0x10480, 0x104AF],
    // 107
    &[0x10800, 0x1083F],
    // 108
    &[0x10A00, 0x10A5F],
    // 109
    &[0x1D300, 0x1D35F],
    // 110
    &[0x12000, 0x1247F],
    // 111
    &[0x1D360, 0x1D37F],
    // 112
    &[0x1B80, 0x1BBF],
    // 113
    &[0x1C00, 0x1C4F],
    // 114
    &[0x1C50, 0x1C7F],
    // 115
    &[0xA880, 0xA8DF],
    // 116
    &[0xA900, 0xA92F],
    // 117
    &[0xA930, 0xA95F],
    // 118
    &[0xAA00, 0xAA5F],
    // 119
    &[0x10190, 0x101CF],
    // 120
    &[0x101D0, 0x101FF],
    // 121
    &[0x10280, 0x102DF, 0x10920, 0x1093F],
    // 122
    &[0x1F000, 0x1F09F],
];

fn build_glyph_ranges(ttf_data: &[u8]) -> imgui::FontGlyphRanges {
    let face = ttf_parser::Face::parse(ttf_data, 0).expect("failed to parse ttf data");
    let unicode_ranges = face.unicode_ranges();

    let mut ranges = vec![];
    for (i, range) in UNICODE_RANGE_TABLE.iter().enumerate() {
        if unicode_ranges.0 & (1u128 << i) != 0 {
            ranges.extend_from_slice(range);
        }
    }
    ranges.push(0);

    imgui::FontGlyphRanges::from_slice(Vec::leak(ranges))
}

fn update_modifiers_io(io: &mut imgui::Io, modifiers: u32) {
    io.add_key_event(imgui::Key::ModCtrl, modifiers & sapp::MODIFIER_CTRL != 0);
    io.add_key_event(imgui::Key::ModShift, modifiers & sapp::MODIFIER_SHIFT != 0);
    io.add_key_event(imgui::Key::ModAlt, modifiers & sapp::MODIFIER_ALT != 0);
    io.add_key_event(imgui::Key::ModSuper, modifiers & sapp::MODIFIER_SUPER != 0);
}

fn mouse_button_to_imgui(btn: sapp::Mousebutton) -> imgui::MouseButton {
    match btn {
        sapp::Mousebutton::Left => imgui::MouseButton::Left,
        sapp::Mousebutton::Right => imgui::MouseButton::Right,
        sapp::Mousebutton::Middle => imgui::MouseButton::Middle,
        _ => imgui::MouseButton::Left,
    }
}

fn map_keycode(keycode: sapp::Keycode) -> Option<imgui::Key> {
    Some(match keycode {
        sapp::Keycode::Space => imgui::Key::Space,
        sapp::Keycode::Apostrophe => imgui::Key::Apostrophe,
        sapp::Keycode::Comma => imgui::Key::Comma,
        sapp::Keycode::Minus => imgui::Key::Minus,
        sapp::Keycode::Period => imgui::Key::Period,
        sapp::Keycode::Slash => imgui::Key::Slash,
        sapp::Keycode::Num0 => imgui::Key::Alpha0,
        sapp::Keycode::Num1 => imgui::Key::Alpha1,
        sapp::Keycode::Num2 => imgui::Key::Alpha2,
        sapp::Keycode::Num3 => imgui::Key::Alpha3,
        sapp::Keycode::Num4 => imgui::Key::Alpha4,
        sapp::Keycode::Num5 => imgui::Key::Alpha5,
        sapp::Keycode::Num6 => imgui::Key::Alpha6,
        sapp::Keycode::Num7 => imgui::Key::Alpha7,
        sapp::Keycode::Num8 => imgui::Key::Alpha8,
        sapp::Keycode::Num9 => imgui::Key::Alpha9,
        sapp::Keycode::Semicolon => imgui::Key::Semicolon,
        sapp::Keycode::Equal => imgui::Key::Equal,
        sapp::Keycode::A => imgui::Key::A,
        sapp::Keycode::B => imgui::Key::B,
        sapp::Keycode::C => imgui::Key::C,
        sapp::Keycode::D => imgui::Key::D,
        sapp::Keycode::E => imgui::Key::E,
        sapp::Keycode::F => imgui::Key::F,
        sapp::Keycode::G => imgui::Key::G,
        sapp::Keycode::H => imgui::Key::H,
        sapp::Keycode::I => imgui::Key::I,
        sapp::Keycode::J => imgui::Key::J,
        sapp::Keycode::K => imgui::Key::K,
        sapp::Keycode::L => imgui::Key::L,
        sapp::Keycode::M => imgui::Key::M,
        sapp::Keycode::N => imgui::Key::N,
        sapp::Keycode::O => imgui::Key::O,
        sapp::Keycode::P => imgui::Key::P,
        sapp::Keycode::Q => imgui::Key::Q,
        sapp::Keycode::R => imgui::Key::R,
        sapp::Keycode::S => imgui::Key::S,
        sapp::Keycode::T => imgui::Key::T,
        sapp::Keycode::U => imgui::Key::U,
        sapp::Keycode::V => imgui::Key::V,
        sapp::Keycode::W => imgui::Key::W,
        sapp::Keycode::X => imgui::Key::X,
        sapp::Keycode::Y => imgui::Key::Y,
        sapp::Keycode::Z => imgui::Key::Z,
        sapp::Keycode::LeftBracket => imgui::Key::LeftBracket,
        sapp::Keycode::Backslash => imgui::Key::Backslash,
        sapp::Keycode::RightBracket => imgui::Key::RightBracket,
        sapp::Keycode::GraveAccent => imgui::Key::GraveAccent,
        sapp::Keycode::Escape => imgui::Key::Escape,
        sapp::Keycode::Enter => imgui::Key::Enter,
        sapp::Keycode::Tab => imgui::Key::Tab,
        sapp::Keycode::Backspace => imgui::Key::Backspace,
        sapp::Keycode::Insert => imgui::Key::Insert,
        sapp::Keycode::Delete => imgui::Key::Delete,
        sapp::Keycode::Right => imgui::Key::RightArrow,
        sapp::Keycode::Left => imgui::Key::LeftArrow,
        sapp::Keycode::Down => imgui::Key::DownArrow,
        sapp::Keycode::Up => imgui::Key::UpArrow,
        sapp::Keycode::PageUp => imgui::Key::PageUp,
        sapp::Keycode::PageDown => imgui::Key::PageDown,
        sapp::Keycode::Home => imgui::Key::Home,
        sapp::Keycode::End => imgui::Key::End,
        sapp::Keycode::CapsLock => imgui::Key::CapsLock,
        sapp::Keycode::ScrollLock => imgui::Key::ScrollLock,
        sapp::Keycode::NumLock => imgui::Key::NumLock,
        sapp::Keycode::PrintScreen => imgui::Key::PrintScreen,
        sapp::Keycode::Pause => imgui::Key::Pause,
        sapp::Keycode::F1 => imgui::Key::F1,
        sapp::Keycode::F2 => imgui::Key::F2,
        sapp::Keycode::F3 => imgui::Key::F3,
        sapp::Keycode::F4 => imgui::Key::F4,
        sapp::Keycode::F5 => imgui::Key::F5,
        sapp::Keycode::F6 => imgui::Key::F6,
        sapp::Keycode::F7 => imgui::Key::F7,
        sapp::Keycode::F8 => imgui::Key::F8,
        sapp::Keycode::F9 => imgui::Key::F9,
        sapp::Keycode::F10 => imgui::Key::F10,
        sapp::Keycode::F11 => imgui::Key::F11,
        sapp::Keycode::F12 => imgui::Key::F12,
        sapp::Keycode::Kp0 => imgui::Key::Keypad0,
        sapp::Keycode::Kp1 => imgui::Key::Keypad1,
        sapp::Keycode::Kp2 => imgui::Key::Keypad2,
        sapp::Keycode::Kp3 => imgui::Key::Keypad3,
        sapp::Keycode::Kp4 => imgui::Key::Keypad4,
        sapp::Keycode::Kp5 => imgui::Key::Keypad5,
        sapp::Keycode::Kp6 => imgui::Key::Keypad6,
        sapp::Keycode::Kp7 => imgui::Key::Keypad7,
        sapp::Keycode::Kp8 => imgui::Key::Keypad8,
        sapp::Keycode::Kp9 => imgui::Key::Keypad9,
        sapp::Keycode::KpDecimal => imgui::Key::KeypadDecimal,
        sapp::Keycode::KpDivide => imgui::Key::KeypadDivide,
        sapp::Keycode::KpMultiply => imgui::Key::KeypadMultiply,
        sapp::Keycode::KpSubtract => imgui::Key::KeypadSubtract,
        sapp::Keycode::KpAdd => imgui::Key::KeypadAdd,
        sapp::Keycode::KpEnter => imgui::Key::KeypadEnter,
        sapp::Keycode::KpEqual => imgui::Key::KeypadEqual,
        sapp::Keycode::LeftShift => imgui::Key::LeftShift,
        sapp::Keycode::LeftControl => imgui::Key::LeftCtrl,
        sapp::Keycode::LeftAlt => imgui::Key::LeftAlt,
        sapp::Keycode::LeftSuper => imgui::Key::LeftSuper,
        sapp::Keycode::RightShift => imgui::Key::RightShift,
        sapp::Keycode::RightControl => imgui::Key::RightCtrl,
        sapp::Keycode::RightAlt => imgui::Key::RightAlt,
        sapp::Keycode::RightSuper => imgui::Key::RightSuper,
        sapp::Keycode::Menu => imgui::Key::Menu,
        _ => return None,
    })
}

fn map_cursor(cursor: imgui::MouseCursor) -> sapp::MouseCursor {
    match cursor {
        imgui::MouseCursor::Arrow => sapp::MouseCursor::Arrow,
        imgui::MouseCursor::TextInput => sapp::MouseCursor::Ibeam,
        imgui::MouseCursor::ResizeAll => sapp::MouseCursor::ResizeAll,
        imgui::MouseCursor::ResizeNS => sapp::MouseCursor::ResizeNs,
        imgui::MouseCursor::ResizeEW => sapp::MouseCursor::ResizeEw,
        imgui::MouseCursor::ResizeNESW => sapp::MouseCursor::ResizeNesw,
        imgui::MouseCursor::ResizeNWSE => sapp::MouseCursor::ResizeNwse,
        imgui::MouseCursor::Hand => sapp::MouseCursor::PointingHand,
        imgui::MouseCursor::NotAllowed => sapp::MouseCursor::NotAllowed,
    }
}
