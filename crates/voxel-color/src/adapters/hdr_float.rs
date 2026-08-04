//! Explicit floating-point HDR output adapter.

use super::DefaultColorAdapter;
use crate::{ColorAdapter, ColorCapabilities, ColorRequest, OutputDepth, ResolvedColorPath};

#[derive(Clone, Copy, Debug, Default)]
pub struct HdrFloatAdapter;

impl ColorAdapter for HdrFloatAdapter {
    fn resolve(
        &self,
        mut request: ColorRequest,
        capabilities: ColorCapabilities,
    ) -> ResolvedColorPath {
        request.depth = OutputDepth::HdrFloat;
        DefaultColorAdapter.resolve(request, capabilities)
    }
}
