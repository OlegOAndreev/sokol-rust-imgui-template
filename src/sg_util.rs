use sokol::gfx as sg;

/// A small wrapper for sg::write_buffer_transient which may resize the buffer if it does not have enough space.
pub fn update_transient_buffer<T>(buf: &mut sg::Buffer, data: &[T]) {
    let cur_usage = sg::query_buffer_usage(*buf);
    assert!(cur_usage.write_transient, "Buffer must be transient");

    let size = size_of_val(data);
    let cur_size = sg::query_buffer_size(*buf);
    if cur_size < size {
        sg::destroy_buffer(*buf);

        // If this is not the first reallocation, add additional capacity to do the amortized linear growth.
        let next_size = if cur_size > 1 { size + size / 4 } else { size };

        *buf = sg::make_buffer(&sg::BufferDesc {
            size: next_size,
            usage: cur_usage,
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
