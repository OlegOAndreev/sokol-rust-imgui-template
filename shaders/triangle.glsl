@vs vs
in vec3 position;
in vec3 color0;
out vec3 color;

void main() {
    gl_Position = vec4(position, 1.0);
    color = color0;
}
@end

@fs fs
in vec3 color;
out vec4 frag_color;

void main() {
    frag_color = vec4(color, 1.0);
}
@end

@program triangle vs fs
