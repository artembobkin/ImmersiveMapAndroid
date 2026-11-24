// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

// The frame's tiles as squares on the flat map, one instance a tile: the
// first thing the engine draws, before it has tiles' contents to draw.

struct Frame {
    // The projection-view matrix the GPU draws with.
    projection_view: mat4x4f,
}

@group(0) @binding(0) var<uniform> frame: Frame;

struct Tile {
    // The tile's south west corner in the flat render world, then its size.
    @location(0) origin_and_size: vec3f,
    @location(1) color: vec4f,
}

struct Out {
    @builtin(position) position: vec4f,
    @location(0) color: vec4f,
    // The position within the tile, 0...1 on both axes.
    @location(1) uv: vec2f,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32, tile: Tile) -> Out {
    let corner = vec2f(f32(index & 1u), f32((index >> 1u) & 1u));
    let world = tile.origin_and_size.xy + corner * tile.origin_and_size.z;
    var out: Out;
    out.position = frame.projection_view * vec4f(world, 0.0, 1.0);
    out.color = tile.color;
    out.uv = corner;
    return out;
}

@fragment
fn fragment(input: Out) -> @location(0) vec4f {
    // A line along the tile's edge, a few pixels wide whatever the tile's size on screen.
    let edge = min(min(input.uv.x, 1.0 - input.uv.x), min(input.uv.y, 1.0 - input.uv.y));
    let width = max(fwidth(input.uv.x), fwidth(input.uv.y)) * 1.5;
    let line = 1.0 - smoothstep(0.0, width, edge);
    return vec4f(mix(input.color.rgb, input.color.rgb * 0.45, line), 1.0);
}
