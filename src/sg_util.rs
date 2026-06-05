use sokol::gfx as sg;

/// A small wrapper for sg::update_buffer which may resize the buffer if it has not enough space.
pub fn update_dynamic_buffer<T: Copy>(buf: &mut sg::Buffer, usage: sg::BufferUsage, data: &[T]) {
    assert!(!usage.immutable, "Immutable buffers cannot be updated");
    let size = size_of_val(data);
    if buf.id != 0 {
        let cur_usage = sg::query_buffer_usage(*buf);
        if sg::query_buffer_size(*buf) < size {
            sg::destroy_buffer(*buf);
            buf.id = 0;
        }
        assert!(
            cur_usage.vertex_buffer == usage.vertex_buffer
                && cur_usage.index_buffer == usage.index_buffer
                && cur_usage.storage_buffer == usage.storage_buffer
                && cur_usage.immutable == usage.immutable
                && cur_usage.dynamic_update == usage.dynamic_update
                && cur_usage.stream_update == usage.stream_update,
            "Trying to update buffer with different usage: {:?} vs {:?}",
            cur_usage,
            usage
        );
    }

    if buf.id == 0 {
        *buf = sg::make_buffer(&sg::BufferDesc {
            size,
            usage,
            ..Default::default()
        });
    }

    sg::update_buffer(
        *buf,
        &sg::Range {
            ptr: data.as_ptr().cast(),
            size,
        },
    );
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
