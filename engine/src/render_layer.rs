// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The layers a frame is drawn in and their order: a port of the web's
//! `render/core/render-layer.ts` (the Swift `RenderLayerPlanner`). Each
//! subsystem draws the layers it owns; the frame engine runs them in the
//! plan's order, the world's in the world pass, the overlay's after it.

/// A layer of the frame, drawn by the subsystem that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderLayer {
    ShadowCasters,
    FarShadowCasters,
    GroundShadowMask,
    Starfield,
    GlobeVectorSurface,
    GlobeCap,
    PlacementStencil,
    FlatMapSurface,
    FlatMapRoads,
    AppLayers,
    SurfaceLabels,
    BuildingExtrusion,
    SceneModels,
    Horizon,
    PostProcessing,
    Labels,
    TileGrid,
    Pins,
    DebugOverlay,
}

impl RenderLayer {
    /// Drawn in the world pass, over the world's depth and stencil.
    pub fn is_world(self) -> bool {
        use RenderLayer::*;
        matches!(self, Starfield | GlobeVectorSurface | GlobeCap | PlacementStencil | FlatMapSurface | FlatMapRoads | AppLayers | SurfaceLabels | BuildingExtrusion | SceneModels | Horizon)
    }

    /// Drawn in the overlay pass, over the world, with a depth of its own.
    pub fn is_overlay(self) -> bool {
        use RenderLayer::*;
        matches!(self, Labels | TileGrid | Pins | DebugOverlay)
    }
}

/// Why a planned layer is not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderSkipReason {
    TransparentSpace,
    NoSceneModelContent,
    NoAppLayerContent,
    NoLabelContent,
    NoTileGridContent,
    NoPinContent,
    DebugOverlayDisabled,
}

/// The surface the world is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderSurfaceMode {
    Flat,
    Spherical,
}

/// What a frame has to draw, which turns layers of the plan on and off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderPassAvailability {
    pub render_surface_mode: RenderSurfaceMode,
    pub labels_enabled: bool,
    pub pins_enabled: bool,
    pub debug_overlay_enabled: bool,
    pub starfield_enabled: bool,
    pub scene_models_enabled: bool,
    pub app_layers_enabled: bool,
    pub tile_grid_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderLayerPlanItem {
    pub layer: RenderLayer,
    pub enabled: bool,
    pub skip_reason: Option<RenderSkipReason>,
}

impl RenderLayerPlanItem {
    fn new(layer: RenderLayer, skip_reason: Option<RenderSkipReason>) -> RenderLayerPlanItem {
        RenderLayerPlanItem { layer, enabled: skip_reason.is_none(), skip_reason }
    }

    fn gated(layer: RenderLayer, enabled: bool, reason: RenderSkipReason) -> RenderLayerPlanItem {
        RenderLayerPlanItem::new(layer, (!enabled).then_some(reason))
    }
}

/// The frame's layers in the order they are drawn, each with whether it is.
pub fn plan_render_layers(a: &RenderPassAvailability) -> Vec<RenderLayerPlanItem> {
    use RenderLayer::*;
    let world: &[RenderLayer] = match a.render_surface_mode {
        // The ground, what stands on it, what lies on it, the horizon last.
        RenderSurfaceMode::Flat => &[PlacementStencil, FlatMapSurface, BuildingExtrusion, SceneModels, FlatMapRoads, AppLayers, SurfaceLabels, Horizon],
        // Sky first, the tile geometry blends over it, the horizon last.
        RenderSurfaceMode::Spherical => &[Starfield, PlacementStencil, GlobeVectorSurface, AppLayers, SurfaceLabels, GlobeCap, SceneModels, Horizon],
    };
    let mut items: Vec<RenderLayerPlanItem> = world
        .iter()
        .map(|&layer| match layer {
            Starfield => RenderLayerPlanItem::gated(layer, a.starfield_enabled, RenderSkipReason::TransparentSpace),
            SceneModels => RenderLayerPlanItem::gated(layer, a.scene_models_enabled, RenderSkipReason::NoSceneModelContent),
            AppLayers => RenderLayerPlanItem::gated(layer, a.app_layers_enabled, RenderSkipReason::NoAppLayerContent),
            _ => RenderLayerPlanItem::new(layer, None),
        })
        .collect();
    items.extend([
        RenderLayerPlanItem::gated(Labels, a.labels_enabled, RenderSkipReason::NoLabelContent),
        RenderLayerPlanItem::gated(TileGrid, a.tile_grid_enabled, RenderSkipReason::NoTileGridContent),
        RenderLayerPlanItem::gated(Pins, a.pins_enabled, RenderSkipReason::NoPinContent),
        RenderLayerPlanItem::gated(DebugOverlay, a.debug_overlay_enabled, RenderSkipReason::DebugOverlayDisabled),
    ]);
    items
}

/// The world's layers in their drawn order: the buildings move before the
/// flat map's surface, so the ground is drawn around what stands on it.
pub fn world_layer_order(layers: &[RenderLayer]) -> Vec<RenderLayer> {
    let mut ordered = layers.to_vec();
    let surface = layers.iter().position(|&layer| layer == RenderLayer::FlatMapSurface);
    let building = layers.iter().position(|&layer| layer == RenderLayer::BuildingExtrusion);
    if let (Some(surface), Some(building)) = (surface, building)
        && surface < building
    {
        let moved = ordered.remove(building);
        ordered.insert(surface, moved);
    }
    ordered
}

/// The frame's enabled layers, split into the world pass's and the overlay pass's.
pub fn pass_layers(availability: &RenderPassAvailability) -> (Vec<RenderLayer>, Vec<RenderLayer>) {
    let enabled: Vec<RenderLayer> = plan_render_layers(availability).into_iter().filter(|item| item.enabled).map(|item| item.layer).collect();
    let world: Vec<RenderLayer> = enabled.iter().copied().filter(|layer| layer.is_world()).collect();
    let overlay = enabled.into_iter().filter(|layer| layer.is_overlay()).collect();
    (world_layer_order(&world), overlay)
}

// Ported from the web's render-layer.test.ts (the Swift `RenderLayerPlannerTests`
// and `RenderPassGraphWorldLayerOrderTests`).
#[cfg(test)]
mod tests {
    use super::RenderLayer::*;
    use super::*;

    const BASE: RenderPassAvailability = RenderPassAvailability {
        render_surface_mode: RenderSurfaceMode::Flat,
        labels_enabled: true,
        pins_enabled: true,
        debug_overlay_enabled: false,
        starfield_enabled: true,
        scene_models_enabled: true,
        app_layers_enabled: true,
        tile_grid_enabled: true,
    };

    fn layers(plan: &[RenderLayerPlanItem]) -> Vec<RenderLayer> {
        plan.iter().map(|item| item.layer).collect()
    }

    #[test]
    fn the_flat_world_draws_the_ground_what_stands_on_it_what_lies_on_it_the_horizon_last() {
        assert_eq!(layers(&plan_render_layers(&BASE)), [PlacementStencil, FlatMapSurface, BuildingExtrusion, SceneModels, FlatMapRoads, AppLayers, SurfaceLabels, Horizon, Labels, TileGrid, Pins, DebugOverlay]);
    }

    #[test]
    fn the_globe_paints_the_sky_first_and_skips_it_with_transparent_space() {
        let plan = plan_render_layers(&RenderPassAvailability { render_surface_mode: RenderSurfaceMode::Spherical, starfield_enabled: false, ..BASE });
        assert_eq!(plan[0], RenderLayerPlanItem { layer: Starfield, enabled: false, skip_reason: Some(RenderSkipReason::TransparentSpace) });
        assert_eq!(layers(&plan[1..8]), [PlacementStencil, GlobeVectorSurface, AppLayers, SurfaceLabels, GlobeCap, SceneModels, Horizon]);
    }

    #[test]
    fn layers_without_content_are_planned_but_disabled_with_their_reason() {
        let plan = plan_render_layers(&RenderPassAvailability { scene_models_enabled: false, app_layers_enabled: false, labels_enabled: false, ..BASE });
        let reason = |layer| plan.iter().find(|item| item.layer == layer).and_then(|item| item.skip_reason);
        assert_eq!(reason(SceneModels), Some(RenderSkipReason::NoSceneModelContent));
        assert_eq!(reason(AppLayers), Some(RenderSkipReason::NoAppLayerContent));
        assert_eq!(reason(Labels), Some(RenderSkipReason::NoLabelContent));
    }

    #[test]
    fn the_tile_grid_draws_over_the_maps_names_and_under_the_pins_and_is_left_out_without_tiles() {
        let plan = plan_render_layers(&RenderPassAvailability { tile_grid_enabled: false, ..BASE });
        assert_eq!(layers(&plan[plan.len() - 4..]), [Labels, TileGrid, Pins, DebugOverlay]);
        assert_eq!(plan.iter().find(|item| item.layer == TileGrid), Some(&RenderLayerPlanItem { layer: TileGrid, enabled: false, skip_reason: Some(RenderSkipReason::NoTileGridContent) }));
    }

    #[test]
    fn the_buildings_move_before_the_ground_in_the_drawn_order() {
        assert_eq!(world_layer_order(&[PlacementStencil, FlatMapSurface, BuildingExtrusion, FlatMapRoads]), [PlacementStencil, BuildingExtrusion, FlatMapSurface, FlatMapRoads]);
        assert_eq!(world_layer_order(&[Starfield, GlobeVectorSurface]), [Starfield, GlobeVectorSurface]);
        // On the globe the models stay where the sphere's order puts them.
        assert_eq!(world_layer_order(&[Starfield, PlacementStencil, GlobeVectorSurface, GlobeCap, SceneModels, Horizon]), [Starfield, PlacementStencil, GlobeVectorSurface, GlobeCap, SceneModels, Horizon]);
    }

    #[test]
    fn the_passes_take_their_enabled_layers_in_the_drawn_order() {
        let (world, overlay) = pass_layers(&RenderPassAvailability { scene_models_enabled: false, labels_enabled: false, ..BASE });
        assert_eq!(world, [PlacementStencil, BuildingExtrusion, FlatMapSurface, FlatMapRoads, AppLayers, SurfaceLabels, Horizon]);
        assert_eq!(overlay, [TileGrid, Pins]);
    }
}
