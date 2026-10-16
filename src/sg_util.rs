#![allow(dead_code)]

use sokol::gfx as sg;

/// A small wrapper for sg::write_buffer_transient which replaces the buffer contents and may resize the buffer if it
/// does not have enough space.
pub fn update_buffer_transient<T>(buf: &mut sg::Buffer, data: &[T]) {
    let size = size_of_val(data);
    assert!(size > 0, "Cannot write an empty slice");
    let desc = sg::query_buffer_desc(*buf);
    assert!(desc.usage.write_transient, "Buffer must be transient");

    if desc.size < size {
        sg::destroy_buffer(*buf);

        // If this is not the first reallocation, add additional capacity to do the amortized linear growth.
        let next_size = if desc.size > 1 { size + size / 4 } else { size };

        *buf = sg::make_buffer(&sg::BufferDesc {
            size: next_size,
            usage: desc.usage,
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
            buffer: *buf,
            offset: 0,
        },
        size,
    });
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

/// A queue of staging buffers: data is written into staging buffers using write_to_buffer() and write_to_image() and
/// later copied into other resources (buffers or images) within the same frame using commit(). commit() may be called
/// multiple times per frame. At the start of the new frame reset() must be called to re-use the allocated staged
/// buffers.
pub struct WriteQueue {
    chunks: Vec<Chunk>,
    buffer_copies: Vec<sg::CopyBufferToBufferDesc>,
    image_copies: Vec<sg::CopyBufferToImageDesc>,
}

struct Chunk {
    buffer: sg::Buffer,
    capacity: usize,
    offset: usize,
    committed: bool,
}

impl WriteQueue {
    /// Capacity of a chunk which is shared by multiple requests.
    pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;
    /// Make all offsets and sizes aligned. This is over-alignment for regular buffers, but works correctly for images
    /// (see _sg_block_bytesize and _sg_pixelformat_bytesize).
    pub const OFFSET_ALIGNMENT: usize = 16;

    pub fn new() -> Self {
        Self {
            chunks: vec![],
            buffer_copies: vec![],
            image_copies: vec![],
        }
    }

    /// Destroy all chunks, must be called before sg::shutdown().
    pub fn destroy(&mut self) {
        for chunk in self.chunks.drain(..) {
            sg::destroy_buffer(chunk.buffer);
        }
        self.buffer_copies.clear();
        self.image_copies.clear();
    }

    /// Returns the total capacity of allocated buffers.
    pub fn capacity(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.capacity).sum()
    }

    /// Frees unused allocated buffers. NOTE: Calling this after reset() will clear ALL the buffers
    pub fn clear(&mut self) {
        self.chunks.retain(|chunk| {
            if chunk.offset == 0 {
                sg::destroy_buffer(chunk.buffer);
                return false;
            }
            true
        });
    }

    /// Reset the queue, must be called at the start of the frame.
    pub fn reset(&mut self) {
        if !self.buffer_copies.is_empty() || !self.image_copies.is_empty() {
            eprintln!(
                "Warning: {} buffer copies and {} image copies staged but not flushed",
                self.buffer_copies.len(),
                self.image_copies.len()
            );
            self.buffer_copies.clear();
            self.image_copies.clear();
        }

        for chunk in &mut self.chunks {
            chunk.offset = 0;
            chunk.committed = false;
        }
    }

    /// Stage data to copy into the destination buffer.
    pub fn write_to_buffer<T>(&mut self, data: &[T], dst: sg::BufferLocation) {
        let src = self.stage(data);
        self.buffer_copies.push(sg::CopyBufferToBufferDesc {
            src,
            dst,
            size: size_of_val(data),
        });
    }

    /// Stage `data` for a copy into the destination image. The data is assumed to be tightly packed.
    pub fn write_to_image<T>(&mut self, data: &[T], dst: sg::ImageLocation, size: sg::ImageExtent) {
        let bytes_per_slice = size_of_val(data) as i32;
        assert!(
            bytes_per_slice % size.height == 0,
            "Image data must tightly packed, but {} is not divisible by height {}",
            bytes_per_slice,
            size.height
        );
        let bytes_per_row = bytes_per_slice / size.height;
        let src = self.stage(data);
        self.image_copies.push(sg::CopyBufferToImageDesc {
            src: sg::BufferImageLocation {
                buffer: src.buffer,
                offset: src.offset,
                bytes_per_row,
                bytes_per_slice,
            },
            dst,
            size,
        });
    }

    /// Run the copies which have been queued since the last flush. Must be called in the same frame in which the data
    /// was staged.
    pub fn commit(&mut self) {
        for copy in self.buffer_copies.drain(..) {
            sg::copy_buffer_to_buffer(&copy)
        }
        for copy in self.image_copies.drain(..) {
            sg::copy_buffer_to_image(&copy)
        }
        for chunk in &mut self.chunks {
            if chunk.offset > 0 {
                chunk.committed = true;
            }
        }
    }

    fn stage<T>(&mut self, data: &[T]) -> sg::BufferLocation {
        let size = size_of_val(data);
        assert!(size > 0, "Cannot write an empty slice");
        let rounded_size = size.next_multiple_of(Self::OFFSET_ALIGNMENT);

        // Find the first chunk which has enough space and was not committed, or allocate a new one.
        let chunk_idx = match self
            .chunks
            .iter()
            .position(|chunk| !chunk.committed && chunk.offset + rounded_size <= chunk.capacity)
        {
            None => {
                let capacity = rounded_size.max(Self::CHUNK_SIZE);

                let buffer = sg::make_buffer(&sg::BufferDesc {
                    size: capacity,

                    usage: sg::BufferUsage {
                        staging_buffer: true,

                        write_transient: true,

                        copy_src: true,

                        ..Default::default()
                    },

                    ..Default::default()
                });

                self.chunks.push(Chunk {
                    buffer,

                    capacity,

                    offset: 0,

                    committed: false,
                });

                self.chunks.len() - 1
            }

            Some(idx) => idx,
        };

        let chunk = &mut self.chunks[chunk_idx];
        let offset = chunk.offset;
        chunk.offset += rounded_size;
        sg::write_buffer_transient(&sg::WriteBufferDesc {
            src: sg::WriteBufferSource {
                data: sg::Range {
                    ptr: data.as_ptr().cast(),
                    size,
                },
                offset: 0,
            },
            dst: sg::BufferLocation {
                buffer: chunk.buffer,
                offset,
            },
            size,
        });

        sg::BufferLocation {
            buffer: chunk.buffer,
            offset,
        }
    }
}
