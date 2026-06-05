@vs vs
layout(binding=0) uniform vs_params {
    vec2 disp_size;
};
in vec2 position;
in vec2 texcoord;
in vec4 color;
out vec4 v_color;
out vec2 uv;

void main() {
    gl_Position = vec4(((position / disp_size) - vec2(0.5)) * vec2(2.0, -2.0), 0.5, 1.0);
    v_color = color;
    uv = texcoord;
}
@end

@fs fs
layout(binding=0) uniform texture2D tex;
layout(binding=0) uniform sampler smp;
in vec4 v_color;
in vec2 uv;
out vec4 frag_color;

void main() {
    frag_color = texture(sampler2D(tex, smp), uv) * v_color;
}
@end

@program imgui vs fs
