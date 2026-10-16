#![allow(dead_code)]

/// egui renderer and event-handler for sokol-app and sokol-gfx: the egui counterpart of `imgui_sokol.rs`.
use crate::imgui_sokol::{
    FONT_TTF_DATA, is_copy_command, is_cut_command, is_macos, is_paste_command, is_printable_char,
};
use crate::{sg_util, shaders::imgui_shader};
use egui::epaint::Primitive;
use sokol::{app as sapp, gfx as sg, time as stm};
use std::collections::HashMap;
use std::sync::Arc;

const FONT_SIZE: f32 = 14.0;

struct Texture {
    image: sg::Image,
    view: sg::View,
    sampler: sg::Sampler,
    options: egui::TextureOptions,
}

pub struct EguiSokol {
    context: egui::Context,
    shader: sg::Shader,
    pipeline: sg::Pipeline,
    vertex_buf: sg::Buffer,
    index_buf: sg::Buffer,
    write_queue: sg_util::WriteQueue,
    textures: HashMap<egui::TextureId, Texture>,
    /// Textures that egui freed in the previous frame, destroyed at the start of the next frame (see
    /// `apply_textures`).
    pending_free: Vec<egui::TextureId>,
    /// The frame's meshes (see `meshes()`), concatenated into `vertices`/`indices` when uploading.
    primitives: Vec<egui::epaint::ClippedPrimitive>,
    vertices: Vec<egui::epaint::Vertex>,
    indices: Vec<u32>,
    // Input state, accumulated until the next frame starts.
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    focused: bool,
    cur_dpi_scale: f32,
    screen_size: [f32; 2],
    /// The device's limit, queried once in setup() because egui wants it with every frame.
    max_texture_side: Option<usize>,
    delayed_set_clipboard: Option<String>,
}

impl EguiSokol {
    /// Create a new (uninitialized) EguiSokol instance. Call setup() before all other methods.
    pub fn new() -> Self {
        Self {
            context: egui::Context::default(),
            shader: sg::Shader::new(),
            pipeline: sg::Pipeline::new(),
            vertex_buf: sg::Buffer::new(),
            index_buf: sg::Buffer::new(),
            write_queue: sg_util::WriteQueue::new(),
            textures: HashMap::new(),
            pending_free: vec![],
            vertices: vec![],
            indices: vec![],
            primitives: vec![],
            events: vec![],
            modifiers: egui::Modifiers::default(),
            focused: true,
            cur_dpi_scale: 1.0,
            screen_size: [1.0, 1.0],
            max_texture_side: None,
            delayed_set_clipboard: None,
        }
    }

    /// Create all required sg resources, must be called once after sg::setup().
    pub fn setup(&mut self) {
        self.setup_style();
        self.max_texture_side = Some(sg::query_limits().max_image_size_2d as usize);

        self.vertex_buf = sg::make_buffer(&sg::BufferDesc {
            size: 1,
            usage: sg::BufferUsage {
                vertex_buffer: true,
                write_transient: true,
                ..Default::default()
            },
            ..Default::default()
        });
        self.index_buf = sg::make_buffer(&sg::BufferDesc {
            size: 1,
            usage: sg::BufferUsage {
                index_buffer: true,
                write_transient: true,
                ..Default::default()
            },
            ..Default::default()
        });

        // egui's vertex layout (position, uv, color) is identical to the imgui one, so the imgui shader can be reused
        // as is.
        self.shader = sg::make_shader(&imgui_shader::imgui_shader_desc(sg::query_backend()));
        let mut layout = sg::VertexLayoutState::new();
        layout.attrs[imgui_shader::ATTR_IMGUI_POSITION] = sg::VertexAttrState {
            buffer_index: 0,
            offset: std::mem::offset_of!(egui::epaint::Vertex, pos) as i32,
            format: sg::VertexFormat::Float2,
        };
        layout.attrs[imgui_shader::ATTR_IMGUI_TEXCOORD] = sg::VertexAttrState {
            buffer_index: 0,
            offset: std::mem::offset_of!(egui::epaint::Vertex, uv) as i32,
            format: sg::VertexFormat::Float2,
        };
        layout.attrs[imgui_shader::ATTR_IMGUI_COLOR] = sg::VertexAttrState {
            buffer_index: 0,
            offset: std::mem::offset_of!(egui::epaint::Vertex, color) as i32,
            format: sg::VertexFormat::Ubyte4n,
        };

        let mut pipeline_desc = sg::PipelineDesc {
            shader: self.shader,
            layout,
            index_type: sg::IndexType::Uint32,
            primitive_type: sg::PrimitiveType::Triangles,
            color_count: 1,
            ..Default::default()
        };
        // egui uses premultiplied-alpha colors, hence One/OneMinusSrcAlpha instead of the straight-alpha blending the
        // imgui pipeline uses.
        pipeline_desc.colors[0] = sg::ColorTargetState {
            write_mask: sg::ColorMask::Rgb,
            blend: sg::BlendState {
                enabled: true,
                src_factor_rgb: sg::BlendFactor::One,
                dst_factor_rgb: sg::BlendFactor::OneMinusSrcAlpha,
                op_rgb: sg::BlendOp::Add,
                src_factor_alpha: sg::BlendFactor::OneMinusDstAlpha,
                dst_factor_alpha: sg::BlendFactor::One,
                op_alpha: sg::BlendOp::Add,
            },
            ..Default::default()
        };
        self.pipeline = sg::make_pipeline(&pipeline_desc);
    }

    /// Shut egui down and release all sg resources.
    pub fn shutdown(&mut self) {
        for (_, texture) in self.textures.drain() {
            destroy_texture(texture);
        }
        self.pending_free.clear();

        self.write_queue.destroy();
        sg::destroy_pipeline(self.pipeline);
        self.pipeline = sg::Pipeline::new();
        sg::destroy_shader(self.shader);
        self.shader = sg::Shader::new();

        sg::destroy_buffer(self.vertex_buf);
        self.vertex_buf = sg::Buffer::new();
        sg::destroy_buffer(self.index_buf);
        self.index_buf = sg::Buffer::new();

        self.vertices.clear();
        self.indices.clear();
        self.primitives.clear();
    }

    pub fn context(&self) -> &egui::Context {
        &self.context
    }

    pub fn context_mut(&mut self) -> &mut egui::Context {
        &mut self.context
    }

    /// Run one egui frame: run `ui` against the context and upload the geometry and images for `render()`. Must be
    /// called outside of an sg render pass.
    ///
    /// The other allowed style of calling a manual begin_pass()/end_pass() pair breaks a few things like text selection
    /// in labels.
    pub fn run_frame(
        &mut self,
        width: i32,
        height: i32,
        delta_time: f64,
        dpi_scale: f32,
        ui: impl FnMut(&mut egui::Ui),
    ) {
        let raw_input = self.make_raw_input(width, height, delta_time, dpi_scale);
        let mut full_output = self.context.run_ui(raw_input, ui);

        self.handle_platform_output(&full_output.platform_output);
        self.apply_textures(&mut full_output.textures_delta);
        self.primitives = self
            .context
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        self.upload_geometry();
    }

    /// Update the screen size and the dpi scale, and create the input of the next pass, including all buffered events.
    fn make_raw_input(
        &mut self,
        width: i32,
        height: i32,
        delta_time: f64,
        dpi_scale: f32,
    ) -> egui::RawInput {
        assert!(width > 0 && height > 0);

        self.cur_dpi_scale = if dpi_scale > 0.0 { dpi_scale } else { 1.0 };
        self.screen_size = [
            width as f32 / self.cur_dpi_scale,
            height as f32 / self.cur_dpi_scale,
        ];

        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                self.screen_size.into(),
            )),
            time: Some(stm::sec(stm::now())),
            predicted_dt: delta_time as f32,
            focused: self.focused,
            ..Default::default()
        };
        raw_input.events.append(&mut self.events);
        raw_input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(self.cur_dpi_scale);
        raw_input.max_texture_side = self.max_texture_side;

        raw_input
    }

    /// Process the egui commands.
    fn handle_platform_output(&self, platform_output: &egui::PlatformOutput) {
        for command in &platform_output.commands {
            match command {
                egui::OutputCommand::CopyText(text) => sapp::set_clipboard_string(text),
                egui::OutputCommand::CopyImage(_) => {
                    eprintln!("egui-sokol: copying images to the clipboard is not supported");
                }
                egui::OutputCommand::OpenUrl(url) => {
                    eprintln!("egui-sokol: opening urls is not supported ({})", url.url);
                }
            }
        }

        // The imgui backend sets its own mouse cursor every frame, so only override it while the pointer is over an
        // egui window (otherwise hovering an imgui widget would end up showing egui's default cursor).
        if pointer_over_egui_pane(&self.context) {
            sapp::set_mouse_cursor(map_cursor(platform_output.cursor_icon));
        }
    }

    /// Create and update the sg textures that egui asks for, and destroy the textures that egui freed in the previous
    /// frame.
    fn apply_textures(&mut self, delta: &mut egui::TexturesDelta) {
        self.write_queue.reset();

        // Destroy the textures that egui freed in the previous frame.
        for id in self.pending_free.drain(..) {
            if let Some(texture) = self.textures.remove(&id) {
                sg::destroy_sampler(texture.sampler);
                sg::destroy_view(texture.view);
                sg::destroy_image(texture.image);
            }
        }

        // Defer this frame's frees until after it has been rendered.
        self.pending_free.extend(delta.free.drain());

        // Do a drain, so that the TexturesDelta destructor does not complain to us (why does it even do that?)
        for (id, deltas) in delta.set.drain() {
            for delta in deltas {
                // Either create texture, recreate texture or update the sampler.
                match delta.pos {
                    None => {
                        if let Some(prev_texture) = self.textures.remove(&id) {
                            destroy_texture(prev_texture);
                        }
                        let image = make_image(delta.image.size());
                        self.textures.insert(
                            id,
                            Texture {
                                image,
                                view: make_view(image),
                                sampler: make_sampler(&delta.options),
                                options: delta.options,
                            },
                        );
                    }
                    Some(_) => {
                        let texture = self
                            .textures
                            .get_mut(&id)
                            .expect("unknown texture ID in deltas");
                        if texture.options != delta.options {
                            texture.sampler = make_sampler(&delta.options);
                            texture.options = delta.options;
                        }
                    }
                }

                // Update the texture data.
                let texture = &self.textures[&id];
                let egui::ImageData::Color(image) = &delta.image;
                let dst_x = delta.pos.map_or(0, |p| p[0]);
                let dst_y = delta.pos.map_or(0, |p| p[1]);

                self.write_queue.write_to_image(
                    &image.pixels,
                    sg::ImageLocation {
                        image: texture.image,
                        mip_level: 0,
                        x: dst_x as i32,
                        y: dst_y as i32,
                        slice: 0,
                    },
                    sg::ImageExtent {
                        width: image.width() as i32,
                        height: image.height() as i32,
                        num_slices: 1,
                    },
                );
            }
        }

        self.write_queue.commit();
    }

    /// Concatenate the frame's meshes into two big vertex and index buffers, so that the whole frame is uploaded in
    /// two sg calls.
    fn upload_geometry(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        for primitive in &self.primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                // Paint callbacks are not supported in this backend.
                continue;
            };
            self.vertices.extend_from_slice(&mesh.vertices);
            self.indices.extend_from_slice(&mesh.indices);
        }

        if !self.vertices.is_empty() {
            sg_util::update_buffer_transient(&mut self.vertex_buf, &self.vertices);
            sg_util::update_buffer_transient(&mut self.index_buf, &self.indices);
        }
    }

    /// Renders all egui draw data, must be called inside a sg render pass, after run_frame().
    pub fn render(&self) {
        // Skip rendering if there is no UI.
        if self.primitives.is_empty() {
            return;
        }

        sg::push_debug_group("egui-sokol");

        let fb_width = (self.screen_size[0] * self.cur_dpi_scale) as i32;
        let fb_height = (self.screen_size[1] * self.cur_dpi_scale) as i32;
        sg::apply_viewport(0, 0, fb_width, fb_height, true);

        sg::apply_pipeline(self.pipeline);
        let vs_params = imgui_shader::VsParams {
            disp_size: self.screen_size,
            _pad_8: [0; _],
        };
        sg_util::apply_uniforms(imgui_shader::UB_VS_PARAMS, &vs_params);

        let mut bind = sg::Bindings::new();
        bind.vertex_buffers[0] = self.vertex_buf;
        bind.index_buffer = self.index_buf;

        let mut cur_texture_id = None;
        let mut cur_vertex_offset = None;
        // sg::draw() has no base_vertex parameter, so each mesh's vertex range is selected here via the vertex buffer
        // offset, derived from the sizes of the meshes uploaded before it.
        let mut next_vertex_offset = 0;
        let mut next_index_offset = 0;
        for primitive in &self.primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                // Paint callbacks are not supported in this backend.
                continue;
            };

            let clip_rect = primitive.clip_rect;
            let vertex_offset = next_vertex_offset;
            let index_offset = next_index_offset;
            next_vertex_offset += size_of_val(mesh.vertices.as_slice()) as i32;
            next_index_offset += mesh.indices.len();

            // Reapply bindings if the texture or vertex_offset has changed.
            if cur_texture_id != Some(mesh.texture_id) || cur_vertex_offset != Some(vertex_offset) {
                let Some(texture) = self.textures.get(&mesh.texture_id) else {
                    eprintln!("egui-sokol: Unknown texture id {:?}", mesh.texture_id);
                    continue;
                };
                bind.views[imgui_shader::VIEW_TEX] = texture.view;
                bind.samplers[imgui_shader::SMP_SMP] = texture.sampler;
                cur_texture_id = Some(mesh.texture_id);
                bind.vertex_buffer_offsets[0] = vertex_offset;
                cur_vertex_offset = Some(vertex_offset);
                sg::apply_bindings(&bind);
            }

            // egui clip rects are in points, scissor rects are in framebuffer pixels.
            let scissor_x = ((clip_rect.min.x * self.cur_dpi_scale).round() as i32).min(fb_width);
            let scissor_y = ((clip_rect.min.y * self.cur_dpi_scale).round() as i32).min(fb_height);
            let scissor_w = (((clip_rect.max.x - clip_rect.min.x) * self.cur_dpi_scale).round()
                as i32)
                .min(fb_width - scissor_x);
            let scissor_h = (((clip_rect.max.y - clip_rect.min.y) * self.cur_dpi_scale).round()
                as i32)
                .min(fb_height - scissor_y);
            sg::apply_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h, true);
            sg::draw(index_offset, mesh.indices.len(), 1);
        }

        // Reset scissor
        sg::apply_scissor_rect(0, 0, fb_width, fb_height, true);
        sg::pop_debug_group();
    }

    /// Handle sapp event, returns true if egui wants to capture keyboard/mouse input.
    pub fn handle_event(&mut self, event: &sapp::Event) -> bool {
        let dpi_scale = self.cur_dpi_scale;
        let mouse_pos = egui::pos2(event.mouse_x / dpi_scale, event.mouse_y / dpi_scale);
        let touch_pos = || {
            egui::pos2(
                event.touches[0].pos_x / dpi_scale,
                event.touches[0].pos_y / dpi_scale,
            )
        };

        match event._type {
            sapp::EventType::KeyDown | sapp::EventType::KeyUp => {
                let pressed = event._type == sapp::EventType::KeyDown;
                self.update_modifiers(event.modifiers);
                if let Some(key) = map_keycode(event.key_code) {
                    if pressed && self.handle_shortcuts(event) {
                        // The key press turned into a dedicated shortcut event.
                    } else {
                        self.events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: event.key_repeat,
                            modifiers: self.modifiers,
                        });
                    }
                }
            }
            sapp::EventType::Char => {
                self.update_modifiers(event.modifiers);
                // Skip characters that are a side effect of a command (e.g. Cmd+C emits a 'c', and Delete emits a
                // control character on macOS).
                if (event.modifiers & (sapp::MODIFIER_CTRL | sapp::MODIFIER_SUPER)) == 0
                    && let Some(ch) = char::from_u32(event.char_code)
                    && is_printable_char(ch)
                {
                    self.events.push(egui::Event::Text(ch.to_string()));
                }
            }
            sapp::EventType::MouseDown | sapp::EventType::MouseUp => {
                self.update_modifiers(event.modifiers);
                self.events.push(egui::Event::PointerButton {
                    pos: mouse_pos,
                    button: map_mouse_button(event.mouse_button),
                    pressed: event._type == sapp::EventType::MouseDown,
                    modifiers: self.modifiers,
                });
            }
            sapp::EventType::MouseScroll => {
                self.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(event.scroll_x, event.scroll_y),
                    phase: egui::TouchPhase::Move,
                    modifiers: self.modifiers,
                });
            }
            sapp::EventType::MouseMove => {
                self.events.push(egui::Event::PointerMoved(mouse_pos));
            }
            sapp::EventType::MouseLeave => {
                // See the same workaround in sokol_imgui.h.
                if cfg!(target_os = "emscripten") {
                    for button in [
                        egui::PointerButton::Primary,
                        egui::PointerButton::Secondary,
                        egui::PointerButton::Middle,
                    ] {
                        self.events.push(egui::Event::PointerButton {
                            pos: mouse_pos,
                            button,
                            pressed: false,
                            modifiers: self.modifiers,
                        });
                    }
                }
                self.events.push(egui::Event::PointerGone);
            }
            sapp::EventType::TouchesBegan => {
                if event.num_touches > 0 {
                    self.events.push(egui::Event::PointerMoved(touch_pos()));
                    self.events.push(egui::Event::PointerButton {
                        pos: touch_pos(),
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: self.modifiers,
                    });
                }
            }
            sapp::EventType::TouchesMoved => {
                if event.num_touches > 0 {
                    self.events.push(egui::Event::PointerMoved(touch_pos()));
                }
            }
            sapp::EventType::TouchesEnded | sapp::EventType::TouchesCancelled => {
                // The last touch might already be gone from the event, fall back to the mouse position that sokol keeps
                // in sync with the first touch.
                let pos = if event.num_touches > 0 {
                    touch_pos()
                } else {
                    mouse_pos
                };
                self.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: self.modifiers,
                });
            }
            sapp::EventType::Focused => {
                self.focused = true;
                self.events.push(egui::Event::WindowFocused(true));
            }
            sapp::EventType::Unfocused => {
                self.focused = false;
                // Avoid sticky modifiers when focus is lost (egui clears its own copy too).
                self.modifiers = egui::Modifiers::default();
                self.events.push(egui::Event::WindowFocused(false));
            }
            sapp::EventType::ClipboardPasted => {
                let contents = sapp::get_clipboard_string().replace("\r\n", "\n");
                if !contents.is_empty() {
                    self.events.push(egui::Event::Paste(contents));
                }
            }
            sapp::EventType::Invalid
            | sapp::EventType::MouseEnter
            | sapp::EventType::Resized
            | sapp::EventType::Iconified
            | sapp::EventType::Restored
            | sapp::EventType::Suspended
            | sapp::EventType::Resumed
            | sapp::EventType::QuitRequested
            | sapp::EventType::FilesDropped
            | sapp::EventType::Num => {}
        }

        self.context.egui_wants_keyboard_input() || self.context.egui_wants_pointer_input()
    }

    /// Configure the fonts and the theme of the UI.
    fn setup_style(&mut self) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "droid_sans".to_owned(),
            Arc::new(egui::FontData::from_static(FONT_TTF_DATA)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.insert(family, vec!["droid_sans".to_owned()]);
        }
        self.context.set_fonts(fonts);

        // Dark theme with white text.
        self.context.set_theme(egui::ThemePreference::Dark);
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(egui::Color32::WHITE);
        visuals.text_cursor.blink = false;
        self.context.set_visuals_of(egui::Theme::Dark, visuals);
        self.context.style_mut_of(egui::Theme::Dark, |style| {
            style.text_styles.insert(
                egui::TextStyle::Small,
                egui::FontId::proportional(FONT_SIZE - 3.0),
            );
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(FONT_SIZE));
            style.text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::proportional(FONT_SIZE),
            );
            style.text_styles.insert(
                egui::TextStyle::Heading,
                egui::FontId::proportional(FONT_SIZE + 4.0),
            );
            style.text_styles.insert(
                egui::TextStyle::Monospace,
                egui::FontId::monospace(FONT_SIZE),
            );
        });
    }

    /// Update the tracked modifier state. egui wants the current modifier state both in the individual events and as a
    /// "modifiers changed" event.
    fn update_modifiers(&mut self, sapp_modifiers: u32) {
        let modifiers = egui::Modifiers {
            alt: sapp_modifiers & sapp::MODIFIER_ALT != 0,
            ctrl: sapp_modifiers & sapp::MODIFIER_CTRL != 0,
            shift: sapp_modifiers & sapp::MODIFIER_SHIFT != 0,
            mac_cmd: is_macos() && sapp_modifiers & sapp::MODIFIER_SUPER != 0,
            command: if is_macos() {
                sapp_modifiers & sapp::MODIFIER_SUPER != 0
            } else {
                sapp_modifiers & sapp::MODIFIER_CTRL != 0
            },
        };
        if modifiers != self.modifiers {
            self.modifiers = modifiers;
            self.events
                .push(egui::Event::ModifiersChanged(self.modifiers));
        }
    }

    /// Translate the clipboard shortcuts (Ctrl/Cmd+C/X/V) into the dedicated egui events (unlike imgui, egui expects us
    /// to translates key presses into dedicated events ourselves). Returns true if the key press was turned into a
    /// shortcut event.
    fn handle_shortcuts(&mut self, event: &sapp::Event) -> bool {
        if is_cut_command(event) {
            self.events.push(egui::Event::Cut);
            true
        } else if is_copy_command(event) {
            self.events.push(egui::Event::Copy);
            true
        } else if is_paste_command(event) {
            // See comments in imgui_sokol.rs for why we skip Ctrl/Cmd+V and instead rely on ClipboardPasted event.
            true
        } else {
            false
        }
    }
}

fn make_image(size: [usize; 2]) -> sg::Image {
    sg::make_image(&sg::ImageDesc {
        width: size[0] as i32,
        height: size[1] as i32,
        pixel_format: sg::PixelFormat::Rgba8,
        usage: sg::ImageUsage {
            copy_dst: true,
            ..Default::default()
        },
        ..Default::default()
    })
}

fn make_view(image: sg::Image) -> sg::View {
    sg::make_view(&sg::ViewDesc {
        texture: sg::TextureViewDesc {
            image,
            ..Default::default()
        },
        ..Default::default()
    })
}

fn make_sampler(options: &egui::TextureOptions) -> sg::Sampler {
    sg::make_sampler(&sg::SamplerDesc {
        min_filter: make_filter(options.minification),
        mag_filter: make_filter(options.magnification),
        wrap_u: make_wrap(options.wrap_mode),
        wrap_v: make_wrap(options.wrap_mode),
        ..Default::default()
    })
}

fn make_filter(filter: egui::TextureFilter) -> sg::Filter {
    match filter {
        egui::TextureFilter::Nearest => sg::Filter::Nearest,
        egui::TextureFilter::Linear => sg::Filter::Linear,
    }
}

fn make_wrap(wrap: egui::TextureWrapMode) -> sg::Wrap {
    match wrap {
        egui::TextureWrapMode::ClampToEdge => sg::Wrap::ClampToEdge,
        egui::TextureWrapMode::Repeat => sg::Wrap::Repeat,
        egui::TextureWrapMode::MirroredRepeat => sg::Wrap::MirroredRepeat,
    }
}

fn map_mouse_button(btn: sapp::Mousebutton) -> egui::PointerButton {
    match btn {
        sapp::Mousebutton::Left => egui::PointerButton::Primary,
        sapp::Mousebutton::Right => egui::PointerButton::Secondary,
        sapp::Mousebutton::Middle => egui::PointerButton::Middle,
        sapp::Mousebutton::Invalid => egui::PointerButton::Primary,
    }
}

fn map_keycode(keycode: sapp::Keycode) -> Option<egui::Key> {
    Some(match keycode {
        sapp::Keycode::Space => egui::Key::Space,
        sapp::Keycode::Apostrophe => egui::Key::Quote,
        sapp::Keycode::Comma => egui::Key::Comma,
        sapp::Keycode::Minus => egui::Key::Minus,
        sapp::Keycode::Period => egui::Key::Period,
        sapp::Keycode::Slash => egui::Key::Slash,
        sapp::Keycode::Num0 => egui::Key::Num0,
        sapp::Keycode::Num1 => egui::Key::Num1,
        sapp::Keycode::Num2 => egui::Key::Num2,
        sapp::Keycode::Num3 => egui::Key::Num3,
        sapp::Keycode::Num4 => egui::Key::Num4,
        sapp::Keycode::Num5 => egui::Key::Num5,
        sapp::Keycode::Num6 => egui::Key::Num6,
        sapp::Keycode::Num7 => egui::Key::Num7,
        sapp::Keycode::Num8 => egui::Key::Num8,
        sapp::Keycode::Num9 => egui::Key::Num9,
        sapp::Keycode::Semicolon => egui::Key::Semicolon,
        sapp::Keycode::Equal => egui::Key::Equals,
        sapp::Keycode::A => egui::Key::A,
        sapp::Keycode::B => egui::Key::B,
        sapp::Keycode::C => egui::Key::C,
        sapp::Keycode::D => egui::Key::D,
        sapp::Keycode::E => egui::Key::E,
        sapp::Keycode::F => egui::Key::F,
        sapp::Keycode::G => egui::Key::G,
        sapp::Keycode::H => egui::Key::H,
        sapp::Keycode::I => egui::Key::I,
        sapp::Keycode::J => egui::Key::J,
        sapp::Keycode::K => egui::Key::K,
        sapp::Keycode::L => egui::Key::L,
        sapp::Keycode::M => egui::Key::M,
        sapp::Keycode::N => egui::Key::N,
        sapp::Keycode::O => egui::Key::O,
        sapp::Keycode::P => egui::Key::P,
        sapp::Keycode::Q => egui::Key::Q,
        sapp::Keycode::R => egui::Key::R,
        sapp::Keycode::S => egui::Key::S,
        sapp::Keycode::T => egui::Key::T,
        sapp::Keycode::U => egui::Key::U,
        sapp::Keycode::V => egui::Key::V,
        sapp::Keycode::W => egui::Key::W,
        sapp::Keycode::X => egui::Key::X,
        sapp::Keycode::Y => egui::Key::Y,
        sapp::Keycode::Z => egui::Key::Z,
        sapp::Keycode::LeftBracket => egui::Key::OpenBracket,
        sapp::Keycode::Backslash => egui::Key::Backslash,
        sapp::Keycode::RightBracket => egui::Key::CloseBracket,
        sapp::Keycode::GraveAccent => egui::Key::Backtick,
        sapp::Keycode::Escape => egui::Key::Escape,
        sapp::Keycode::Enter => egui::Key::Enter,
        sapp::Keycode::Tab => egui::Key::Tab,
        sapp::Keycode::Backspace => egui::Key::Backspace,
        sapp::Keycode::Insert => egui::Key::Insert,
        sapp::Keycode::Delete => egui::Key::Delete,
        sapp::Keycode::Right => egui::Key::ArrowRight,
        sapp::Keycode::Left => egui::Key::ArrowLeft,
        sapp::Keycode::Down => egui::Key::ArrowDown,
        sapp::Keycode::Up => egui::Key::ArrowUp,
        sapp::Keycode::PageUp => egui::Key::PageUp,
        sapp::Keycode::PageDown => egui::Key::PageDown,
        sapp::Keycode::Home => egui::Key::Home,
        sapp::Keycode::End => egui::Key::End,
        sapp::Keycode::F1 => egui::Key::F1,
        sapp::Keycode::F2 => egui::Key::F2,
        sapp::Keycode::F3 => egui::Key::F3,
        sapp::Keycode::F4 => egui::Key::F4,
        sapp::Keycode::F5 => egui::Key::F5,
        sapp::Keycode::F6 => egui::Key::F6,
        sapp::Keycode::F7 => egui::Key::F7,
        sapp::Keycode::F8 => egui::Key::F8,
        sapp::Keycode::F9 => egui::Key::F9,
        sapp::Keycode::F10 => egui::Key::F10,
        sapp::Keycode::F11 => egui::Key::F11,
        sapp::Keycode::F12 => egui::Key::F12,
        sapp::Keycode::Kp0 => egui::Key::Num0,
        sapp::Keycode::Kp1 => egui::Key::Num1,
        sapp::Keycode::Kp2 => egui::Key::Num2,
        sapp::Keycode::Kp3 => egui::Key::Num3,
        sapp::Keycode::Kp4 => egui::Key::Num4,
        sapp::Keycode::Kp5 => egui::Key::Num5,
        sapp::Keycode::Kp6 => egui::Key::Num6,
        sapp::Keycode::Kp7 => egui::Key::Num7,
        sapp::Keycode::Kp8 => egui::Key::Num8,
        sapp::Keycode::Kp9 => egui::Key::Num9,
        sapp::Keycode::KpDecimal => egui::Key::Period,
        sapp::Keycode::KpDivide => egui::Key::Slash,
        sapp::Keycode::KpSubtract => egui::Key::Minus,
        sapp::Keycode::KpAdd => egui::Key::Plus,
        sapp::Keycode::KpEnter => egui::Key::Enter,
        sapp::Keycode::KpEqual => egui::Key::Equals,
        sapp::Keycode::Invalid
        | sapp::Keycode::Pause
        | sapp::Keycode::World1
        | sapp::Keycode::World2
        | sapp::Keycode::CapsLock
        | sapp::Keycode::ScrollLock
        | sapp::Keycode::NumLock
        | sapp::Keycode::PrintScreen
        | sapp::Keycode::F13
        | sapp::Keycode::F14
        | sapp::Keycode::F15
        | sapp::Keycode::F16
        | sapp::Keycode::F17
        | sapp::Keycode::F18
        | sapp::Keycode::F19
        | sapp::Keycode::F20
        | sapp::Keycode::F21
        | sapp::Keycode::F22
        | sapp::Keycode::F23
        | sapp::Keycode::F24
        | sapp::Keycode::F25
        | sapp::Keycode::KpMultiply
        // Modified keys are already reported via ModifiersChanged
        | sapp::Keycode::LeftShift
        | sapp::Keycode::LeftControl
        | sapp::Keycode::LeftAlt
        | sapp::Keycode::LeftSuper
        | sapp::Keycode::RightShift
        | sapp::Keycode::RightControl
        | sapp::Keycode::RightAlt
        | sapp::Keycode::RightSuper
        | sapp::Keycode::Menu => return None,
    })
}

/// Whether the pointer is over an egui pane, i.e. a floating area ([`egui::Window`], menu, popup, ...).
fn pointer_over_egui_pane(context: &egui::Context) -> bool {
    if let Some(pos) = context.pointer_interact_pos() {
        if let Some(layer) = context.layer_id_at(pos) {
            if layer.order != egui::Order::Background {
                return true;
            }
        }
    }
    false
}

fn map_cursor(cursor: egui::CursorIcon) -> sapp::MouseCursor {
    match cursor {
        egui::CursorIcon::Default
        | egui::CursorIcon::None
        | egui::CursorIcon::ContextMenu
        | egui::CursorIcon::Help => sapp::MouseCursor::Arrow,
        egui::CursorIcon::PointingHand => sapp::MouseCursor::PointingHand,
        egui::CursorIcon::Progress | egui::CursorIcon::Wait => sapp::MouseCursor::Arrow,
        egui::CursorIcon::Cell | egui::CursorIcon::Crosshair => sapp::MouseCursor::Crosshair,
        egui::CursorIcon::Text | egui::CursorIcon::VerticalText => sapp::MouseCursor::Ibeam,
        egui::CursorIcon::Alias | egui::CursorIcon::Copy => sapp::MouseCursor::Arrow,
        egui::CursorIcon::Move | egui::CursorIcon::AllScroll => sapp::MouseCursor::ResizeAll,
        egui::CursorIcon::NoDrop | egui::CursorIcon::NotAllowed => sapp::MouseCursor::NotAllowed,
        egui::CursorIcon::Grab | egui::CursorIcon::Grabbing => sapp::MouseCursor::PointingHand,
        egui::CursorIcon::ResizeHorizontal
        | egui::CursorIcon::ResizeEast
        | egui::CursorIcon::ResizeWest
        | egui::CursorIcon::ResizeColumn => sapp::MouseCursor::ResizeEw,
        egui::CursorIcon::ResizeNeSw
        | egui::CursorIcon::ResizeSouthWest
        | egui::CursorIcon::ResizeNorthEast => sapp::MouseCursor::ResizeNesw,
        egui::CursorIcon::ResizeNwSe
        | egui::CursorIcon::ResizeSouthEast
        | egui::CursorIcon::ResizeNorthWest => sapp::MouseCursor::ResizeNwse,
        egui::CursorIcon::ResizeVertical
        | egui::CursorIcon::ResizeSouth
        | egui::CursorIcon::ResizeNorth
        | egui::CursorIcon::ResizeRow => sapp::MouseCursor::ResizeNs,
        egui::CursorIcon::ZoomIn | egui::CursorIcon::ZoomOut => sapp::MouseCursor::Crosshair,
    }
}

fn destroy_texture(texture: Texture) {
    sg::destroy_sampler(texture.sampler);
    sg::destroy_view(texture.view);
    sg::destroy_image(texture.image);
}
