//! What world generation actually produced, and a slice through the caves.
//!
//! A toggled window rather than overlay text, for the same reason the settings
//! panel is: a readout that covers the render is measuring the wrong thing.
//!
//! This panel exists because of how this arc's bugs were found. "Caves only form
//! near the surface" and "the world has no water" were both invisible from inside
//! the world — the first needed a throwaway probe printing a per-depth histogram,
//! the second needed reading the terrain formula against the sea level by hand.
//! Both are now two lines in a window: the generated surface range next to the sea
//! level, and the carved cave range next to the authored band. When a number here
//! disagrees with what the graph says, that difference IS the bug.

use egui::{Color32, Sense, Stroke, Vec2};
use voxel_core::cave::{CaveProgram, CaveSurface, CellularAutomataField, CellularFieldCache};
use voxel_core::chunk::{ChunkGenerator, GenerationStats};
use voxel_core::scatter::ScatterClass;
use voxel_core::stage_report::{GenerationStage, LayerSlice, StageReport};
use voxel_core::world::Voxel;

/// Everything the panel reads. Borrowed rather than copied so the panel cannot
/// drift from the program that built the world.
pub struct WorldgenContext<'a> {
    /// The generator the live world was built from, after sanitisation — so the
    /// panel shows the values generation USED, not the ones it was handed.
    pub generator: &'a ChunkGenerator,
    /// Measured during the last build.
    pub stats: &'a GenerationStats,
    /// What each generation stage did, measured during the build.
    pub report: &'a voxel_core::stage_report::StageReport,
    pub world_seed: u64,
}

/// A horizontal slice of the cellular field, drawn as a plan view.
///
/// The CA field is the thing that decides where rooms can exist at all, and it is
/// otherwise completely invisible: by the time it reaches the screen it has been
/// intersected with a 3D density field and buried under rock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceView {
    pub world_y: i32,
    pub show_corridors: bool,
}

pub struct WorldgenPanel {
    /// Toggled by **G**. Starts hidden: the viewport is the point.
    pub visible: bool,
    slice: SliceView,
    /// The field is deterministic, so it is rebuilt only when the slice height or
    /// the program changes rather than every frame. Redrawing an unchanged field
    /// per repaint is what made the Studio cave previews cost milliseconds a frame.
    cached_field: Option<(i32, CaveProgram, u64, CellularAutomataField)>,
}

impl Default for WorldgenPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldgenPanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            slice: SliceView {
                world_y: 20,
                show_corridors: true,
            },
            cached_field: None,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn draw(&mut self, context: &egui::Context, worldgen: WorldgenContext<'_>) {
        if !self.visible {
            return;
        }
        // `Window::open` borrows the flag mutably, which would conflict with the
        // body — so the flag round-trips through a local.
        let mut visible = self.visible;
        egui::Window::new("World Generation  (G)")
            .open(&mut visible)
            .default_width(460.0)
            .default_height(620.0)
            .resizable(true)
            .vscroll(true)
            .show(context, |ui| self.draw_body(ui, worldgen));
        self.visible = visible;
    }

    fn draw_body(&mut self, ui: &mut egui::Ui, worldgen: WorldgenContext<'_>) {
        let WorldgenContext {
            generator,
            stats,
            report,
            world_seed,
        } = worldgen;
        let generator = &generator.sanitized();

        ui.label(format!("seed {world_seed}  ·  {} chunks", stats.chunks));
        ui.separator();

        // The pipeline first, the per-stage prose after it. A reader who opens this
        // window is asking "what did generation do", and the answer to that is a
        // shape and three percentages, not the fourteen numbers below.
        draw_stage_pipeline(ui, report);
        ui.add_space(8.0);
        draw_material_composition(ui, report);
        ui.add_space(8.0);
        draw_layer_chart(ui, report, generator);

        ui.add_space(8.0);
        ui.separator();
        ui.strong("Pass one — terrain");
        // Surface range against the sea level is the whole "why is there no water"
        // question, answered by putting the two numbers side by side.
        match stats.surface_range() {
            Some((lowest, highest)) => {
                ui.label(format!(
                    "surface Y {lowest}..{highest}   sea level {}",
                    generator.sea_level
                ));
                if lowest >= generator.sea_level {
                    ui.colored_label(
                        Color32::from_rgb(230, 170, 60),
                        "no terrain below the sea level: this world cannot have water",
                    );
                }
            }
            None => {
                ui.colored_label(Color32::from_rgb(230, 120, 120), "no terrain generated");
            }
        }
        ui.label(format!(
            "base height {}   relief {}   headroom {}",
            generator.terrain_base_height, generator.terrain_relief, generator.surface_headroom
        ));
        // The sanitised headroom, so a value the world height cannot honour shows
        // as the value generation actually used rather than as a silent clamp.
        ui.label(format!(
            "solid {}   open water {}",
            stats.solid_voxels, stats.open_air_water
        ));

        ui.add_space(6.0);
        ui.strong("Pass one — caves and fluids");
        if !generator.caves_enabled {
            ui.label("caves disabled");
        } else {
            ui.label(format!(
                "authored band Y {}..{}   shape {:?}",
                generator.cave_program.min_y,
                generator.cave_program.max_y,
                generator.cave_program.shape
            ));
            match stats.cave_range() {
                Some((lowest, highest)) => ui.label(format!(
                    "carved Y {lowest}..{highest}   {} voxels",
                    stats.cave_voxels
                )),
                None => ui.colored_label(
                    Color32::from_rgb(230, 170, 60),
                    "the band is authored but nothing carved — check Openness",
                ),
            };
            let fluids = &generator.cave_fluids;
            if fluids.enabled {
                ui.label(format!(
                    "water ≤ Y {}  ({} voxels)   lava ≤ Y {}  ({} voxels)   floor Y {}",
                    fluids.water_level,
                    stats.cave_water,
                    fluids.lava_level,
                    stats.cave_lava,
                    fluids.min_y
                ));
            } else {
                ui.label("underground fluids disabled");
            }
        }

        ui.add_space(6.0);
        ui.strong("Pass two — scatter");
        if !generator.scatter.enabled {
            ui.label("scatter disabled");
        } else {
            for class in ScatterClass::ALL {
                let rule = generator
                    .scatter
                    .rules
                    .iter()
                    .find(|rule| rule.class == class);
                let rate = rule.map(|rule| rule.chance).unwrap_or(0.0);
                ui.label(format!(
                    "{:<14} {:>7}   rate {:.3}",
                    class.label(),
                    stats.scatter.count(class),
                    rate
                ));
            }
            ui.label(format!("total {}", stats.scatter.total()));
        }

        ui.add_space(6.0);
        ui.strong("Cost");
        ui.label(format!(
            "cellular fields built {}   (one per region per {} -voxel band)",
            stats.cellular_fields_built,
            voxel_core::cave::CELLULAR_Y_BAND_SIZE
        ));

        ui.add_space(8.0);
        ui.separator();
        ui.strong("Cellular field slice");
        let span = generator.min_y..generator.min_y + generator.height as i32;
        ui.horizontal(|ui| {
            ui.label("Y");
            ui.add(egui::Slider::new(
                &mut self.slice.world_y,
                span.start..=span.end - 1,
            ));
            ui.checkbox(&mut self.slice.show_corridors, "corridors");
        });
        self.draw_slice(ui, generator);
    }

    /// The plan view: one pixel block per CA cell, walls dark and open cells light,
    /// with the carved room-to-room corridors drawn over the top.
    fn draw_slice(&mut self, ui: &mut egui::Ui, generator: &ChunkGenerator) {
        let program = &generator.cave_program;
        let needs_rebuild = match &self.cached_field {
            Some((cached_y, cached_program, cached_seed, _)) => {
                *cached_y != self.slice.world_y
                    || cached_program != program
                    || *cached_seed != generator.world_seed
            }
            None => true,
        };
        if needs_rebuild {
            let mut cache = CellularFieldCache::new();
            let field = cache
                .field_at(program, generator.world_seed, self.slice.world_y, 0, 0)
                .clone();
            self.cached_field = Some((
                self.slice.world_y,
                program.clone(),
                generator.world_seed,
                field,
            ));
        }
        let Some((_, _, _, field)) = &self.cached_field else {
            return;
        };

        const CELLS: usize = 64;
        let available = ui.available_width().clamp(128.0, 384.0);
        let cell_size = available / CELLS as f32;
        let (response, painter) =
            ui.allocate_painter(Vec2::splat(CELLS as f32 * cell_size), Sense::hover());
        let origin = response.rect.min;
        let cell_voxels = program.cellular_cell_size.max(1) as i32;
        for row in 0..CELLS {
            for column in 0..CELLS {
                let world_x = column as i32 * cell_voxels;
                let world_z = row as i32 * cell_voxels;
                let color = if field.is_wall(world_x, world_z) {
                    Color32::from_rgb(30, 28, 34)
                } else {
                    Color32::from_rgb(150, 160, 175)
                };
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        origin + Vec2::new(column as f32 * cell_size, row as f32 * cell_size),
                        Vec2::splat(cell_size),
                    ),
                    0.0,
                    color,
                );
            }
        }
        if self.slice.show_corridors {
            // Corridors are the part of the recipe that makes a field playable
            // rather than a set of sealed pockets, so they are worth seeing
            // separately from the rooms they join.
            let stroke = Stroke::new(1.5, Color32::from_rgb(240, 170, 70));
            for (start, end) in field.corridor_links() {
                painter.line_segment(
                    [
                        origin
                            + Vec2::new(
                                (start.0 as f32 + 0.5) * cell_size,
                                (start.1 as f32 + 0.5) * cell_size,
                            ),
                        origin
                            + Vec2::new(
                                (end.0 as f32 + 0.5) * cell_size,
                                (end.1 as f32 + 0.5) * cell_size,
                            ),
                    ],
                    stroke,
                );
            }
        }
        let open_cells = (0..CELLS)
            .flat_map(|row| (0..CELLS).map(move |column| (column, row)))
            .filter(|&(column, row)| {
                !field.is_wall(column as i32 * cell_voxels, row as i32 * cell_voxels)
            })
            .count();
        ui.label(format!(
            "{open_cells} of {} cells open   {} corridors   cell size {cell_voxels}",
            CELLS * CELLS,
            field.corridor_links().len()
        ));
        if self.slice.world_y < program.min_y || self.slice.world_y > program.max_y {
            ui.colored_label(
                Color32::from_rgb(230, 170, 60),
                "this height is outside the authored cave band, so nothing carves here",
            );
        }
    }
}

/// A stage's authored colour as an egui colour.
///
/// The mapping lives in `voxel-core` so that a stage is the same colour in this
/// window, in the Studio preview and in any future readout: a legend that has to
/// be re-learned per view is worse than no legend.
fn stage_color(stage: GenerationStage) -> Color32 {
    let [red, green, blue] = stage.color();
    Color32::from_rgb(red, green, blue)
}

/// Which stage's output a finished material IS.
///
/// The histogram counts the FINISHED world, where a voxel no longer remembers who
/// wrote it, so the stage is recovered from what the voxel is. Water and lava are
/// attributed to `Fluids` even though the open-air ocean is Terrain's work: the
/// layer view in `voxel-core` makes exactly the same choice for exactly the same
/// reason — the reader is looking for fluid, and splitting it by author would put
/// the sea in the rock's colour.
///
/// `None` is air, which no stage produces: Terrain leaves it, Carve reveals it.
/// Colouring it as either would overstate one of them by roughly half a world.
fn material_stage(voxel: Voxel) -> Option<GenerationStage> {
    if matches!(voxel, Voxel::Water | Voxel::Lava) {
        Some(GenerationStage::Fluids)
    } else if voxel.is_scatter_cover() {
        // Before the solid test: a cave crystal is both decoration and rock, and
        // the stage that placed it is the more useful of the two answers.
        Some(GenerationStage::Scatter)
    } else if voxel.is_solid() {
        Some(GenerationStage::Terrain)
    } else if voxel == Voxel::Air {
        None
    } else {
        // Non-solid, non-cover plant life: reeds, lily pads, glow berries. Pass two
        // placed them even though `is_scatter_cover` does not claim them.
        Some(GenerationStage::Scatter)
    }
}

/// The bar colour for one material row.
fn material_row_color(voxel: Voxel) -> Color32 {
    match material_stage(voxel) {
        Some(stage) => stage_color(stage),
        None => Color32::from_gray(96),
    }
}

/// How much of a bar's full width one value earns, in `[0, 1]`.
///
/// Zero-safe because an empty world is a real state this window has to render —
/// the "nothing carved" case is one of the two bugs the panel exists to show.
fn bar_fraction(value: u32, maximum: u32) -> f32 {
    if maximum == 0 {
        0.0
    } else {
        (value as f32 / maximum as f32).clamp(0.0, 1.0)
    }
}

/// One painted row of the layer chart: a contiguous run of world layers, summed.
///
/// The chart has more layers than it has pixels of height — a world column is 64
/// voxels and the widget is a few hundred pixels wide but only tens tall once it
/// shares a window with everything else. Summing adjacent layers into bands keeps
/// every voxel in the picture; sampling every n-th layer would drop the thin water
/// table and the one-voxel scatter crust, which are the two features worth seeing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LayerBand {
    lowest_y: i32,
    highest_y: i32,
    solid: u32,
    carved: u32,
    fluid: u32,
    scatter: u32,
}

impl LayerBand {
    fn total(&self) -> u32 {
        self.solid + self.carved + self.fluid + self.scatter
    }
}

/// Fold `layers` into at most `max_rows` bands, lowest first.
///
/// Bands are cut by index rather than by world Y so the row count is exact and no
/// band is empty: `layers` is contiguous by construction, and a chart with a
/// ragged final row reads as missing data.
fn layer_bands(layers: &[LayerSlice], max_rows: usize) -> Vec<LayerBand> {
    if layers.is_empty() || max_rows == 0 {
        return Vec::new();
    }
    let rows = layers.len().min(max_rows);
    (0..rows)
        .map(|row| {
            let start = row * layers.len() / rows;
            let end = (row + 1) * layers.len() / rows;
            let slice = &layers[start..end];
            let mut band = LayerBand {
                lowest_y: slice[0].world_y,
                highest_y: slice[slice.len() - 1].world_y,
                ..LayerBand::default()
            };
            for layer in slice {
                band.solid += layer.solid;
                band.carved += layer.carved;
                band.fluid += layer.water + layer.lava;
                band.scatter += layer.scatter;
            }
            band
        })
        .collect()
}

/// A proportional bar, drawn on its own row at the current cursor.
fn draw_bar(ui: &mut egui::Ui, fraction: f32, width: f32, color: Color32) {
    let (response, painter) = ui.allocate_painter(Vec2::new(width, 9.0), Sense::hover());
    let track = response.rect;
    painter.rect_filled(track, 1.0, Color32::from_gray(38));
    let filled = track.width() * fraction.clamp(0.0, 1.0);
    if filled > 0.0 {
        painter.rect_filled(
            egui::Rect::from_min_size(track.min, Vec2::new(filled, track.height())),
            1.0,
            color,
        );
    }
}

/// A small filled square in a stage's colour, for use as a legend swatch.
fn draw_swatch(ui: &mut egui::Ui, color: Color32) {
    let (response, painter) = ui.allocate_painter(Vec2::splat(10.0), Sense::hover());
    painter.rect_filled(response.rect, 1.0, color);
}

/// The stage strip: what fraction of what each step looked at, it changed.
///
/// A hit rate is the number an author is actually tuning. "Openness 0.45" means
/// nothing on its own; "carve changed 13% of the 32 768 voxels it considered" is
/// the same slider expressed as its effect, and it is what makes a mis-scaled
/// property obvious without generating a world and walking into it.
///
/// A stage that never ran keeps its row and says so. An omitted row is
/// indistinguishable from a readout that forgot the stage exists, which is the
/// failure this whole window was built to avoid.
fn draw_stage_pipeline(ui: &mut egui::Ui, report: &StageReport) {
    ui.strong("Pipeline — what each stage applied");
    egui::Grid::new("worldgen-stage-pipeline")
        .num_columns(5)
        .spacing([8.0, 3.0])
        .show(ui, |ui| {
            for stage in GenerationStage::ALL {
                let count = report.stage(stage);
                draw_swatch(ui, stage_color(stage));
                ui.label(format!("{} (pass {})", stage.label(), stage.pass()));
                if count.considered == 0 {
                    ui.colored_label(Color32::from_gray(150), "not run");
                    ui.label("");
                    ui.label("");
                } else {
                    ui.label(format!("{} / {}", count.changed, count.considered));
                    ui.label(format!("{:>5.1}%", count.hit_rate() * 100.0));
                    draw_bar(ui, count.hit_rate(), 120.0, stage_color(stage));
                }
                ui.end_row();
            }
        });
}

/// The composition list: what the finished world is made of.
///
/// Air is included and the denominator is stated. Dropping air makes every other
/// percentage a share of the rock rather than of the world, which reads as the same
/// kind of number and is off by a factor of two or more — a trap worth avoiding by
/// construction rather than by footnote.
fn draw_material_composition(ui: &mut egui::Ui, report: &StageReport) {
    /// Enough rows to reach past the soils into the decoration, few enough that the
    /// list does not become the window.
    const ROWS: usize = 10;

    let ranked = report.materials.ranked();
    let total = report.materials.total();
    ui.strong("Composition — what the world is made of");
    if total == 0 {
        ui.colored_label(Color32::from_rgb(230, 120, 120), "nothing generated");
        return;
    }
    ui.label(format!(
        "{total} voxels counted, air included  ·  {} materials present",
        ranked.len()
    ));
    let widest = ranked.first().map(|(_, count)| *count).unwrap_or(0);
    egui::Grid::new("worldgen-material-composition")
        .num_columns(4)
        .spacing([8.0, 3.0])
        .show(ui, |ui| {
            for (voxel, count) in ranked.iter().take(ROWS) {
                // The `Voxel` debug name rather than the material table's: naming a
                // row through `voxel-material` would make this panel depend on the
                // renderer's material identity to print a word.
                ui.label(format!("{voxel:?}"));
                ui.label(format!("{count:>8}"));
                ui.label(format!("{:>5.2}%", *count as f32 * 100.0 / total as f32));
                draw_bar(
                    ui,
                    bar_fraction(*count, widest),
                    140.0,
                    material_row_color(*voxel),
                );
                ui.end_row();
            }
        });
    if ranked.len() > ROWS {
        let shown: u32 = ranked.iter().take(ROWS).map(|(_, count)| *count).sum();
        ui.label(format!(
            "{} further materials, {:.2}% between them",
            ranked.len() - ROWS,
            (total - shown) as f32 * 100.0 / total as f32
        ));
    }
}

/// The vertical layer chart: the pipeline's shape, depth by depth.
///
/// This is the view the arc's bugs were found with. A cave system is not legible
/// from a total — "13% carved" is equally consistent with a proper cave network and
/// with a thin sheet of void under the grass. Stacked per depth, the rock visibly
/// thins where the rooms are, the water table shows up as a flat band with a hard
/// top, and the scatter crust sits in a couple of rows at the surface. The two bugs
/// this window was built for ("caves only near the surface", "no water anywhere")
/// are both a glance at this chart.
///
/// Y increases upward, as in the world. A chart of depths drawn top-down reads
/// inverted against every other view of the same world, including the one out of
/// the window.
fn draw_layer_chart(ui: &mut egui::Ui, report: &StageReport, generator: &ChunkGenerator) {
    /// One row per world layer where the widget can afford it; bands above that.
    const MAX_ROWS: usize = 72;
    const ROW_HEIGHT: f32 = 6.0;
    /// Room for a `-128`-width Y tick to the left of the bars.
    const GUTTER: f32 = 38.0;

    ui.strong("Layers — the pipeline by depth");
    let bands = layer_bands(report.layers(), MAX_ROWS);
    let Some((lowest_band, highest_band)) = bands.first().zip(bands.last()) else {
        ui.colored_label(Color32::from_gray(150), "no layer data in this build");
        return;
    };
    let lowest_y = lowest_band.lowest_y;
    let highest_y = highest_band.highest_y;
    let widest = bands.iter().map(LayerBand::total).max().unwrap_or(0);
    if widest == 0 {
        ui.colored_label(Color32::from_gray(150), "every layer is empty");
        return;
    }

    ui.horizontal_wrapped(|ui| {
        ui.label("stacked:");
        for (stage, name) in [
            (GenerationStage::Terrain, "solid"),
            (GenerationStage::Carve, "carved void"),
            (GenerationStage::Fluids, "water + lava"),
            (GenerationStage::Scatter, "decoration"),
        ] {
            draw_swatch(ui, stage_color(stage));
            ui.label(name);
        }
    });

    let bar_width = (ui.available_width() - GUTTER).clamp(160.0, 420.0);
    let (response, painter) = ui.allocate_painter(
        Vec2::new(GUTTER + bar_width, bands.len() as f32 * ROW_HEIGHT),
        Sense::hover(),
    );
    let chart = response.rect;
    painter.rect_filled(chart, 2.0, Color32::from_rgb(12, 14, 18));
    let bar_left = chart.left() + GUTTER;

    for (row, band) in bands.iter().enumerate() {
        // Row zero is the lowest layer, painted at the bottom edge.
        let top = chart.bottom() - (row + 1) as f32 * ROW_HEIGHT;
        let mut cursor = bar_left;
        for (value, stage) in [
            (band.solid, GenerationStage::Terrain),
            (band.carved, GenerationStage::Carve),
            (band.fluid, GenerationStage::Fluids),
            (band.scatter, GenerationStage::Scatter),
        ] {
            if value == 0 {
                continue;
            }
            let width = bar_fraction(value, widest) * bar_width;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(cursor, top + 0.5),
                    Vec2::new(width.max(0.75), ROW_HEIGHT - 1.0),
                ),
                0.0,
                stage_color(stage),
            );
            cursor += width;
        }
        if band.lowest_y % 16 == 0 {
            painter.text(
                egui::pos2(bar_left - 4.0, top + ROW_HEIGHT * 0.5),
                egui::Align2::RIGHT_CENTER,
                format!("{}", band.lowest_y),
                egui::FontId::monospace(9.0),
                Color32::from_gray(150),
            );
        }
    }

    // Guides in world Y, mapped continuously rather than per band: a guide snapped
    // to a band boundary would claim the sea level sits where the chart's row grid
    // happens to fall.
    let span = (highest_y - lowest_y + 1) as f32;
    let screen_y =
        |world_y: i32| chart.bottom() - (world_y - lowest_y) as f32 / span * chart.height();
    let mut guides = vec![(
        generator.sea_level,
        "sea level",
        Color32::from_rgb(96, 176, 220),
    )];
    if generator.caves_enabled {
        guides.push((
            generator.cave_program.min_y,
            "cave band floor",
            Color32::from_rgb(210, 170, 90),
        ));
        guides.push((
            generator.cave_program.max_y,
            "cave band roof",
            Color32::from_rgb(210, 170, 90),
        ));
    }
    for (world_y, label, color) in guides {
        if !(lowest_y..=highest_y + 1).contains(&world_y) {
            continue;
        }
        let y = screen_y(world_y);
        painter.line_segment(
            [egui::pos2(bar_left, y), egui::pos2(chart.right(), y)],
            Stroke::new(1.0, color),
        );
        painter.text(
            egui::pos2(chart.right() - 3.0, y - 1.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{label} Y {world_y}"),
            egui::FontId::monospace(9.0),
            color,
        );
    }
    painter.rect_stroke(
        chart,
        2.0,
        Stroke::new(1.0, Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );
    ui.label(format!(
        "Y {lowest_y}..{highest_y} in {} rows  ·  widest layer {widest} voxels",
        bands.len()
    ));
}

/// A human label for a classified cave surface, for tooltips and diagnostics.
pub const fn cave_surface_label(surface: CaveSurface) -> &'static str {
    match surface {
        CaveSurface::Interior => "interior",
        CaveSurface::Floor => "floor",
        CaveSurface::Ceiling => "ceiling",
        CaveSurface::Wall => "wall",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel must report the world that exists. A generator whose terrain
    /// never reaches below its sea level cannot have water, and that is exactly
    /// the state the panel calls out — so the check the UI performs is pinned here
    /// rather than only being visible by eye.
    #[test]
    fn a_world_without_a_coastline_is_detectable_from_the_stats() {
        let dry = ChunkGenerator {
            terrain_base_height: 48,
            terrain_relief: 4,
            sea_level: 32,
            ..ChunkGenerator::default()
        };
        let (lowest, _) = dry.surface_range();
        assert!(lowest >= dry.sea_level, "this fixture must be the dry case");

        let coastal = ChunkGenerator::default();
        let (lowest, highest) = coastal.surface_range();
        assert!(
            lowest < coastal.sea_level && highest > coastal.sea_level,
            "the default world must have a coastline the panel reports as healthy"
        );
    }

    /// A finished voxel must land on the stage that produced it, because the
    /// composition list colours its bars by this and a mis-classified row makes the
    /// legend actively misleading.
    #[test]
    fn a_finished_voxel_is_attributed_to_the_stage_that_produced_it() {
        assert_eq!(material_stage(Voxel::Stone), Some(GenerationStage::Terrain));
        assert_eq!(material_stage(Voxel::Water), Some(GenerationStage::Fluids));
        assert_eq!(material_stage(Voxel::Lava), Some(GenerationStage::Fluids));
        assert_eq!(
            material_stage(Voxel::TallGrass),
            Some(GenerationStage::Scatter)
        );
        // Solid AND decoration: the stage that placed it wins over what it is made of.
        assert!(Voxel::CaveCrystal.is_solid());
        assert_eq!(
            material_stage(Voxel::CaveCrystal),
            Some(GenerationStage::Scatter)
        );
        // Air belongs to no stage: Terrain leaves it and Carve reveals it, so
        // charging it to either would overstate that stage by half a world.
        assert_eq!(material_stage(Voxel::Air), None);
    }

    #[test]
    fn a_bar_is_zero_safe_and_clamped() {
        assert_eq!(bar_fraction(0, 0), 0.0);
        assert_eq!(bar_fraction(7, 0), 0.0);
        assert!((bar_fraction(3, 4) - 0.75).abs() < 1e-6);
        assert_eq!(bar_fraction(9, 4), 1.0);
    }

    /// Bucketing must conserve every voxel and cover every world Y: the chart's
    /// whole claim is that it shows the world's vertical structure, and a fold that
    /// drops rows would quietly hide the thin features it exists to reveal.
    #[test]
    fn layer_bands_conserve_counts_and_cover_every_height() {
        let layers: Vec<LayerSlice> = (0..8)
            .map(|offset| LayerSlice {
                world_y: offset,
                solid: 1,
                carved: 2,
                water: 3,
                lava: 4,
                scatter: 5,
            })
            .collect();
        let bands = layer_bands(&layers, 4);
        assert_eq!(bands.len(), 4);
        assert_eq!(bands[0].lowest_y, 0);
        assert_eq!(bands[0].highest_y, 1);
        assert_eq!(bands[3].highest_y, 7);
        // Water and lava share the Fluids segment, matching how `voxel-core`'s own
        // layer view groups them.
        assert_eq!(bands[0].fluid, (3 + 4) * 2);
        assert_eq!(
            bands.iter().map(LayerBand::total).sum::<u32>(),
            layers.iter().map(LayerSlice::total).sum::<u32>()
        );
        // Bands are contiguous, so no world Y falls between two rows.
        for pair in bands.windows(2) {
            assert_eq!(pair[0].highest_y + 1, pair[1].lowest_y);
        }
    }

    #[test]
    fn layer_bands_degrade_to_one_row_per_layer_and_handle_emptiness() {
        let layers: Vec<LayerSlice> = (0..3)
            .map(|offset| LayerSlice {
                world_y: offset - 1,
                solid: 1,
                ..LayerSlice::default()
            })
            .collect();
        let bands = layer_bands(&layers, 32);
        assert_eq!(bands.len(), 3);
        assert_eq!(bands[0].lowest_y, -1);
        assert_eq!(bands[0].highest_y, -1);
        assert!(layer_bands(&[], 8).is_empty());
        assert!(layer_bands(&layers, 0).is_empty());
    }

    /// Every decoration class must have a row in the readout, or a class whose
    /// rate is authored to zero would be indistinguishable from one the panel
    /// forgot to list.
    #[test]
    fn the_panel_lists_every_scatter_class() {
        let generator = ChunkGenerator::default();
        for class in ScatterClass::ALL {
            assert!(
                generator
                    .scatter
                    .rules
                    .iter()
                    .any(|rule| rule.class == class),
                "{} has no default rule, so the panel would show it as absent",
                class.label()
            );
        }
    }
}
