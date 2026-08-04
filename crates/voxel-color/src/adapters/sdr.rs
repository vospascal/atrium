//! Explicit SDR output adapter.

use super::DefaultColorAdapter;
use crate::{ColorAdapter, ColorCapabilities, ColorRequest, OutputDepth, ResolvedColorPath};

#[derive(Clone, Copy, Debug, Default)]
pub struct SdrAdapter;

impl ColorAdapter for SdrAdapter {
    fn resolve(
        &self,
        mut request: ColorRequest,
        capabilities: ColorCapabilities,
    ) -> ResolvedColorPath {
        request.depth = OutputDepth::EightBit;
        DefaultColorAdapter.resolve(request, capabilities)
    }
}
