//! The `recipe` synth: a recipe's YAML (`atrium_recipe::RecipeDef`) and how
//! it configures `crate::synth::recipe::RecipeSource`.
//!
//! The recipe is read as written and checked apart, in [`RecipeSynthParams::check`]:
//! a preset is read through an untagged enum (a generator, else a
//! recording), which would hide why a recipe failed behind "did not match
//! any variant". Checked when the preset is resolved, the error names the
//! preset and the field.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use atrium_recipe::{Recipe, RecipeDef};
use serde::Deserialize;

use crate::synth::recipe::{layers, RecipeSource};

#[derive(Deserialize, Clone, Debug)]
pub(in crate::config) struct RecipeSynthParams {
    #[serde(flatten)]
    pub recipe: RecipeDef,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl RecipeSynthParams {
    /// What is wrong with the recipe, if anything: `chords[1].notes[2]: …`.
    pub(super) fn check(&self) -> Result<(), String> {
        self.resolved().map(drop)
    }

    fn resolved(&self) -> Result<Recipe, String> {
        self.recipe.resolve().map_err(|error| error.to_string())
    }

    /// Construct the generator at `position`. `default_seed` applies when the
    /// preset does not pin one.
    ///
    /// # Panics
    ///
    /// If the recipe is not valid: every preset is checked when it is
    /// resolved (`source::resolve`), before any generator is built from it.
    pub(super) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        let recipe = self
            .resolved()
            .unwrap_or_else(|error| panic!("a recipe is checked before it is built: {error}"));
        Box::new(RecipeSource::new(
            position,
            &recipe,
            self.master_gain.unwrap_or(1.0),
            self.seed.unwrap_or(default_seed),
        ))
    }

    /// The chords and their colour, then pings and swoosh when the recipe has
    /// them.
    pub(super) fn layers(&self) -> Vec<(String, String)> {
        let keys = match self.resolved() {
            Ok(recipe) => layers(&recipe),
            Err(_) => vec!["chords", "colour"],
        };
        keys.iter()
            .map(|key| (key.to_string(), key.to_string()))
            .collect()
    }
}
