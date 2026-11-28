// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! ImmersiveMap Web's shaders built into pipelines on this machine's GPU,
//! the driver the engine draws with: on a device, Vulkan and its vendor's
//! compiler. The web's own check (`scripts/check-shaders.mjs`) runs naga's
//! front end alone; this one goes on to the backend and the driver, for
//! each entry point, under the override constants as shipped, each one
//! flipped on its own and all of them flipped together.
//!
//! Modules are put together by the web's rule: a file that starts with
//! `// requires: a.wgsl b.wgsl` is prepended with those files from its own
//! directory, a `// library` file is checked with its includers only, and a
//! module with the clip-distances markers is checked in both variants.
//!
//! The pipelines are stand-ins, not the web's: its real ones are described
//! in its TypeScript, which this test does not read. A fragment stage goes
//! with the module's vertex stage that gives its inputs, the targets take a
//! generic format for each output's type, the depth is `Depth32Float`, the
//! vertex inputs are packed in one buffer and the layout is wgpu's own. So
//! a pass says that each stage compiles through naga's backend and the
//! driver, under those constants; whether the web's pipelines build as
//! the web describes them is checked as each step ports them.
//!
//! Vulkan is asked for, as on a device; `WGPU_BACKEND` asks for another. A
//! GPU is taken over a software renderer, though on a desktop whose GPU
//! wgpu refuses (Haswell's Vulkan) llvmpipe is all there is.
//!
//! The shaders are read from `WEB_SHADERS`, ImmersiveMap Web's
//! `packages/immersive-map/src`, else from a checkout of ImmersiveMap Web
//! next to this repository. With neither the test is skipped, and says so.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use immersive_map_engine::gpu::GpuContext;
use naga::{Binding, ScalarKind, ShaderStage, TypeInner, VectorSize};

fn shader_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("WEB_SHADERS") {
        return Some(PathBuf::from(root));
    }
    let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ImmersiveMapWeb/packages/immersive-map/src");
    sibling.is_dir().then_some(sibling)
}

/// A module as a pipeline of the web compiles it.
struct Module {
    label: String,
    code: String,
    /// The device feature without which the web never builds it.
    needs: Option<wgpu::Features>,
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("a readable directory").map(|entry| entry.expect("an entry").path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "wgsl") {
            files.push(path);
        }
    }
}

/// The lines of the other variant dropped: `clipDistanceVariant` in globe/clip-distance-variant.ts.
fn clip_variant(source: &str, clip_distances: bool) -> String {
    let drop = if clip_distances { "// no-clip-distances" } else { "// clip-distances" };
    source.lines().filter(|line| !line.trim_end().ends_with(drop)).collect::<Vec<_>>().join("\n")
}

fn modules(root: &Path) -> Vec<Module> {
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut modules = Vec::new();
    for file in files {
        let source = std::fs::read_to_string(&file).expect("a readable shader");
        if source.lines().any(|line| line == "// library") {
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(requires) = source.lines().find_map(|line| line.strip_prefix("// requires: ")) {
            for name in requires.split_whitespace() {
                parts.push(std::fs::read_to_string(file.parent().expect("a directory").join(name)).unwrap_or_else(|_| panic!("{} requires {name}", file.display())));
            }
        }
        parts.push(source);
        let full = parts.join("\n");
        let label = file.strip_prefix(root).expect("under the root").display().to_string();
        let mut needs = None;
        // The web prepends `enable subgroups;` to the culler, which naga 30
        // refuses as not implemented: wgpu takes the subgroup builtins
        // without it, on a device with the feature, and so does this port.
        if label.ends_with("model-tile-cull.wgsl") {
            needs = Some(wgpu::Features::SUBGROUP);
        }
        let has_variants = full.lines().any(|line| {
            let line = line.trim_end();
            line.ends_with("// clip-distances") || line.ends_with("// no-clip-distances")
        });
        if has_variants {
            modules.push(Module { label: format!("{label} (clip distances)"), code: clip_variant(&full, true), needs: Some(wgpu::Features::CLIP_DISTANCES | needs.unwrap_or(wgpu::Features::empty())) });
            modules.push(Module { label: format!("{label} (no clip distances)"), code: clip_variant(&full, false), needs });
        } else {
            modules.push(Module { label, code: full, needs });
        }
    }
    modules
}

/// A stage's user-defined inputs or outputs: location, its type and its binding.
type Io = BTreeMap<u32, (TypeInner, Binding)>;

fn collect_io(module: &naga::Module, ty: naga::Handle<naga::Type>, binding: Option<&Binding>, io: &mut Io) {
    match binding {
        Some(binding @ Binding::Location { location, .. }) => {
            io.insert(*location, (module.types[ty].inner.clone(), binding.clone()));
        }
        Some(Binding::BuiltIn(_)) => {}
        None => {
            if let TypeInner::Struct { members, .. } = &module.types[ty].inner {
                for member in members {
                    collect_io(module, member.ty, member.binding.as_ref(), io);
                }
            }
        }
    }
}

fn inputs(module: &naga::Module, entry: &naga::EntryPoint) -> Io {
    let mut io = Io::new();
    for argument in &entry.function.arguments {
        collect_io(module, argument.ty, argument.binding.as_ref(), &mut io);
    }
    io
}

fn outputs(module: &naga::Module, entry: &naga::EntryPoint) -> Io {
    let mut io = Io::new();
    if let Some(result) = &entry.function.result {
        collect_io(module, result.ty, result.binding.as_ref(), &mut io);
    }
    io
}

/// The vertex attribute a vertex input reads.
fn vertex_format(inner: &TypeInner) -> Option<wgpu::VertexFormat> {
    use wgpu::VertexFormat as F;
    let (scalar, size) = match inner {
        TypeInner::Scalar(scalar) => (*scalar, 1),
        TypeInner::Vector { size, scalar } => (*scalar, *size as u8),
        _ => return None,
    };
    Some(match (scalar.kind, scalar.width, size) {
        (ScalarKind::Float, 4, 1) => F::Float32,
        (ScalarKind::Float, 4, 2) => F::Float32x2,
        (ScalarKind::Float, 4, 3) => F::Float32x3,
        (ScalarKind::Float, 4, 4) => F::Float32x4,
        (ScalarKind::Float, 2, 1) => F::Float16,
        (ScalarKind::Float, 2, 2) => F::Float16x2,
        (ScalarKind::Float, 2, _) => F::Float16x4,
        (ScalarKind::Uint, 4, 1) => F::Uint32,
        (ScalarKind::Uint, 4, 2) => F::Uint32x2,
        (ScalarKind::Uint, 4, 3) => F::Uint32x3,
        (ScalarKind::Uint, 4, 4) => F::Uint32x4,
        (ScalarKind::Sint, 4, 1) => F::Sint32,
        (ScalarKind::Sint, 4, 2) => F::Sint32x2,
        (ScalarKind::Sint, 4, 3) => F::Sint32x3,
        (ScalarKind::Sint, 4, 4) => F::Sint32x4,
        _ => return None,
    })
}

/// A colour target a fragment output can be written to.
fn target_format(inner: &TypeInner) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as T;
    let (kind, size) = match inner {
        TypeInner::Scalar(scalar) => (scalar.kind, 1),
        TypeInner::Vector { size, scalar } => (scalar.kind, match size {
            VectorSize::Bi | VectorSize::Tri => 2,
            VectorSize::Quad => 4,
        }),
        _ => return None,
    };
    Some(match (kind, size) {
        (ScalarKind::Float, 1) => T::R16Float,
        (ScalarKind::Float, 2) => T::Rg16Float,
        (ScalarKind::Float, _) => T::Rgba16Float,
        (ScalarKind::Uint, 1) => T::R32Uint,
        (ScalarKind::Uint, 2) => T::Rg32Uint,
        (ScalarKind::Uint, _) => T::Rgba32Uint,
        (ScalarKind::Sint, 1) => T::R32Sint,
        (ScalarKind::Sint, 2) => T::Rg32Sint,
        (ScalarKind::Sint, _) => T::Rgba32Sint,
        _ => return None,
    })
}

/// The override constants to build a stage under: as shipped, each flipped
/// on its own, all flipped together. A flipped `bool` or number is 1 where
/// it ships 0, else 0.
fn constant_sets(module: &naga::Module) -> Vec<(String, Vec<(String, f64)>)> {
    let mut flipped = Vec::new();
    for (_, constant) in module.overrides.iter() {
        let Some(name) = &constant.name else { continue };
        let shipped = constant.init.and_then(|init| match module.global_expressions[init] {
            naga::Expression::Literal(literal) => Some(match literal {
                naga::Literal::Bool(value) => f64::from(u8::from(value)),
                naga::Literal::F32(value) => f64::from(value),
                naga::Literal::U32(value) => f64::from(value),
                naga::Literal::I32(value) => f64::from(value),
                naga::Literal::F16(value) => f64::from(f32::from(value)),
                _ => 0.0,
            }),
            _ => None,
        });
        let value = if shipped.unwrap_or(0.0) == 0.0 { 1.0 } else { 0.0 };
        flipped.push((name.clone(), value));
    }
    let mut sets = vec![("shipped".to_owned(), Vec::new())];
    for (name, value) in &flipped {
        sets.push((format!("{name}={value}"), vec![(name.clone(), *value)]));
    }
    if flipped.len() > 1 {
        sets.push(("all flipped".to_owned(), flipped));
    }
    sets
}

/// The vertex stage a fragment stage is built with: one whose outputs carry
/// each of the fragment's inputs, the one sharing the longest start of its
/// name where several do.
fn vertex_for<'a>(module: &'a naga::Module, fragment: &naga::EntryPoint) -> Option<&'a naga::EntryPoint> {
    let wanted = inputs(module, fragment);
    module
        .entry_points
        .iter()
        .filter(|entry| entry.stage == ShaderStage::Vertex)
        .filter(|vertex| {
            let given = outputs(module, vertex);
            wanted.iter().all(|(location, (inner, binding))| given.get(location).is_some_and(|(given_inner, given_binding)| given_inner == inner && interpolation(given_binding) == interpolation(binding)))
        })
        .max_by_key(|vertex| vertex.name.chars().zip(fragment.name.chars()).take_while(|(a, b)| a == b).count())
}

fn interpolation(binding: &Binding) -> Option<(Option<naga::Interpolation>, Option<naga::Sampling>)> {
    match binding {
        Binding::Location { interpolation, sampling, .. } => Some((*interpolation, *sampling)),
        Binding::BuiltIn(_) => None,
    }
}

struct Check<'a> {
    device: &'a wgpu::Device,
    built: usize,
    failures: Vec<String>,
}

impl Check<'_> {
    /// Runs `build` inside validation and internal error scopes: an error a
    /// scope catches, or a panic, is a failure of `label`.
    fn run(&mut self, label: String, build: impl FnOnce()) {
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)).is_err();
        let internal = pollster::block_on(internal.pop());
        let validation = pollster::block_on(validation.pop());
        let mut why = String::new();
        if panicked {
            why.push_str("panicked; ");
        }
        for error in [internal, validation].into_iter().flatten() {
            let _ = write!(why, "{error}; ");
        }
        if why.is_empty() {
            self.built += 1;
        } else {
            self.failures.push(format!("{label}: {why}"));
        }
    }
}

#[test]
fn the_web_shaders_build_into_pipelines_on_this_gpu() {
    let Some(root) = shader_root() else {
        eprintln!("no web shaders: set WEB_SHADERS to ImmersiveMap Web's packages/immersive-map/src, the shader check is skipped");
        return;
    };
    assert!(root.is_dir(), "WEB_SHADERS is not a directory: {}", root.display());
    // Vulkan, the backend the engine draws with on a device; WGPU_BACKEND picks another.
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(descriptor.with_env());
    // A GPU's driver over a software renderer such as llvmpipe, which a desktop lists too.
    let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    adapters.sort_by_key(|adapter| adapter.get_info().device_type == wgpu::DeviceType::Cpu);
    let Some(adapter) = adapters.into_iter().next() else {
        eprintln!("no GPU adapter, the shader check is skipped");
        return;
    };
    // The device as the engine asks for it: its features and limits.
    let gpu = pollster::block_on(GpuContext::new(&adapter)).expect("a device");
    let features = gpu.device.features();
    let device = gpu.device.clone();
    let info = gpu.adapter_info.clone();
    eprintln!("{}", gpu.report());

    let mut check = Check { device: &device, built: 0, failures: Vec::new() };
    let mut skipped = Vec::new();
    let modules = modules(&root);
    assert!(!modules.is_empty(), "no shaders under {}", root.display());
    for module in &modules {
        if let Some(needs) = module.needs
            && !features.contains(needs)
        {
            skipped.push(format!("{} (no {:?})", module.label, needs - features));
            continue;
        }
        let parsed = match naga::front::wgsl::parse_str(&module.code) {
            Ok(parsed) => parsed,
            Err(error) => {
                check.failures.push(format!("{}: {}", module.label, error.emit_to_string(&module.code)));
                continue;
            }
        };
        let mut shader = None;
        check.run(format!("{} (module)", module.label), || {
            shader = Some(device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(&module.label), source: wgpu::ShaderSource::Wgsl(module.code.as_str().into()) }));
        });
        let Some(shader) = shader else { continue };
        let sets = constant_sets(&parsed);
        let mut paired_vertices = Vec::new();
        let mut stages: Vec<(Option<&naga::EntryPoint>, Option<&naga::EntryPoint>, Option<&naga::EntryPoint>)> = Vec::new();
        for entry in &parsed.entry_points {
            match entry.stage {
                ShaderStage::Fragment => match vertex_for(&parsed, entry) {
                    Some(vertex) => {
                        paired_vertices.push(vertex.name.clone());
                        stages.push((Some(vertex), Some(entry), None));
                    }
                    None => check.failures.push(format!("{} {}: no vertex stage of the module gives its inputs", module.label, entry.name)),
                },
                ShaderStage::Compute => stages.push((None, None, Some(entry))),
                _ => {}
            }
        }
        for entry in parsed.entry_points.iter().filter(|entry| entry.stage == ShaderStage::Vertex && !paired_vertices.contains(&entry.name)) {
            stages.push((Some(entry), None, None));
        }
        for (vertex, fragment, compute) in stages {
            for (set_name, constants) in &sets {
                let constants: Vec<(&str, f64)> = constants.iter().map(|(name, value)| (name.as_str(), *value)).collect();
                let options = wgpu::PipelineCompilationOptions { constants: &constants, zero_initialize_workgroup_memory: true };
                if let Some(compute) = compute {
                    check.run(format!("{} {} [{set_name}]", module.label, compute.name), || {
                        let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(&compute.name), layout: None, module: &shader, entry_point: Some(&compute.name), compilation_options: options.clone(), cache: None });
                    });
                    continue;
                }
                let vertex = vertex.expect("a render pipeline has a vertex stage");
                let mut attributes = Vec::new();
                let mut stride = 0;
                for (location, (inner, _)) in inputs(&parsed, vertex) {
                    let Some(format) = vertex_format(&inner) else {
                        check.failures.push(format!("{} {}: input {location} has no vertex format", module.label, vertex.name));
                        continue;
                    };
                    attributes.push(wgpu::VertexAttribute { format, offset: stride, shader_location: location });
                    stride += format.size().next_multiple_of(4);
                }
                let buffers = [Some(wgpu::VertexBufferLayout { array_stride: stride.max(4), step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes })];
                let targets: Vec<Option<wgpu::ColorTargetState>> = match fragment {
                    Some(fragment) => {
                        let written = outputs(&parsed, fragment);
                        let count = written.keys().next_back().map_or(0, |last| last + 1);
                        (0..count).map(|location| written.get(&location).and_then(|(inner, _)| target_format(inner)).map(Into::into)).collect()
                    }
                    None => Vec::new(),
                };
                let name = match fragment {
                    Some(fragment) => format!("{} + {}", vertex.name, fragment.name),
                    None => vertex.name.clone(),
                };
                check.run(format!("{} {name} [{set_name}]", module.label), || {
                    let _ = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(&name),
                        layout: None,
                        vertex: wgpu::VertexState { module: &shader, entry_point: Some(&vertex.name), compilation_options: options.clone(), buffers: if attributes.is_empty() { &[] } else { &buffers } },
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
                        multisample: wgpu::MultisampleState::default(),
                        fragment: fragment.map(|fragment| wgpu::FragmentState { module: &shader, entry_point: Some(&fragment.name), compilation_options: options.clone(), targets: &targets }),
                        multiview_mask: None,
                        cache: None,
                    });
                });
            }
        }
    }
    eprintln!("{} modules, {} pipelines built, {} failed", modules.len() - skipped.len(), check.built, check.failures.len());
    for module in &skipped {
        eprintln!("skipped: {module}");
    }
    for failure in &check.failures {
        eprintln!("FAIL {failure}");
    }
    assert!(check.failures.is_empty(), "{} of the web's pipelines do not build on {}", check.failures.len(), info.name);
}
