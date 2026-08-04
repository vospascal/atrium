//! Explicit ten-bit output adapter.

use super::DefaultColorAdapter;
use crate::{ColorAdapter, ColorCapabilities, ColorRequest, OutputDepth, ResolvedColorPath};

#[derive(Clone, Copy, Debug, Default)]
pub struct TenBitAdapter;

impl ColorAdapter for TenBitAdapter {
    fn resolve(
        &self,
        mut request: ColorRequest,
        capabilities: ColorCapabilities,
    ) -> ResolvedColorPath {
        request.depth = OutputDepth::TenBit;
        DefaultColorAdapter.resolve(request, capabilities)
    }
}
