use sokol::gfx as sg;

/// A small wrapper for buffer + staging buffer, which can be dynamically re-created if they do not have enough size.
#[derive(PartialEq)]
pub enum DynamicBufferType {
    VERTEX,
    INDEX,
    STORAGE,
}

pub struct DynamicBuffer {
    typ: DynamicBufferType,
    buf: sg::Buffer,
    staging_buf: sg::Buffer,
}

impl DynamicBuffer {
    pub fn new(typ: DynamicBufferType) -> Self {
        Self {
            typ,
            buf: sg::Buffer::new(),
            staging_buf: sg::Buffer::new(),
        }
    }

    /// Destroys buffers, resetting the object to default state.
    pub fn destroy(&mut self) {
        sg::destroy_buffer(self.buf);
        self.buf = sg::Buffer::new();
        sg::destroy_buffer(self.staging_buf);
        self.staging_buf = sg::Buffer::new();
    }

    /// Returns currently allocated buffer (the result must not be used after upload() has been called).
    pub fn buffer(&self) -> sg::Buffer {
        self.buf
    }

    /// Replace the data in the buffer with new data. It must be called at most once per frame and outside of a sg
    /// render pass.
    pub fn upload<T>(&mut self, data: &[T]) {
        let size = size_of_val(data);
        if size == 0 {
            return;
        }

        let first_alloc = self.buf.id == 0;
        // Destroy old buffer if it does not have enough space
        if self.buf.id != 0 {
            if sg::query_buffer_size(self.buf) < size {
                sg::destroy_buffer(self.buf);
                sg::destroy_buffer(self.staging_buf);
                self.buf.id = 0;
                self.staging_buf.id = 0;
            }
        }

        // (Re-)create new buffer
        if self.buf.id == 0 {
            // When creating buffer for the first time use the exact size of the data. If the buffer gets re-created,
            // add some slack for amortized linear growth.
            let new_size = if first_alloc { size } else { size + size / 4 };
            self.buf = sg::make_buffer(&sg::BufferDesc {
                size: new_size,
                usage: sg::BufferUsage {
                    vertex_buffer: self.typ == DynamicBufferType::VERTEX,
                    index_buffer: self.typ == DynamicBufferType::INDEX,
                    storage_buffer: self.typ == DynamicBufferType::STORAGE,
                    copy_dst: true,
                    ..Default::default()
                },
                ..Default::default()
            });
            self.staging_buf = sg::make_buffer(&sg::BufferDesc {
                size: new_size,
                usage: sg::BufferUsage {
                    staging_buffer: self.typ != DynamicBufferType::INDEX,
                    staging_index_buffer: self.typ == DynamicBufferType::INDEX,
                    write_transient: true,
                    copy_src: true,
                    ..Default::default()
                },
                ..Default::default()
            });
        }

        sg::write_buffer_transient(&sg::WriteBufferDesc {
            src: sg::WriteBufferSource {
                data: sg::Range {
                    ptr: data.as_ptr().cast(),
                    size,
                },
                offset: 0,
            },
            dst: sg::BufferLocation {
                buffer: self.staging_buf,
                offset: 0,
            },
            size,
        });
        sg::copy_buffer_to_buffer(&sg::CopyBufferToBufferDesc {
            src: sg::BufferLocation {
                buffer: self.staging_buf,
                offset: 0,
            },
            dst: sg::BufferLocation {
                buffer: self.buf,
                offset: 0,
            },
            size,
        });
    }
}

impl Drop for DynamicBuffer {
    fn drop(&mut self) {
        sg::destroy_buffer(self.buf);
        self.buf = sg::Buffer::new();
        sg::destroy_buffer(self.staging_buf);
        self.staging_buf = sg::Buffer::new();
    }
}

/// A wrapper for sg::apply_uniforms
pub fn apply_uniforms<T>(ub_slot: usize, data: &T) {
    sg::apply_uniforms(
        ub_slot,
        &sg::Range {
            ptr: data as *const _ as *const _,
            size: size_of_val(data),
        },
    );
}
