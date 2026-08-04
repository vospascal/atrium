//! Runtime output-colour state.
//!
//! This is the small piece of state the renderer carries between frames.  Policy lives
//! in a [`ColorAdapter`]; state only holds the user's request, the probed capabilities,
//! and the latest display-headroom reading.

use crate::{ColorAdapter, ColorCapabilities, ColorRequest, DisplayHeadroom, ResolvedColorPath};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorState {
    request: ColorRequest,
    capabilities: ColorCapabilities,
    headroom: DisplayHeadroom,
}

impl Default for ColorState {
    fn default() -> Self {
        Self {
            request: ColorRequest::default(),
            capabilities: ColorCapabilities::default(),
            headroom: DisplayHeadroom::default(),
        }
    }
}

impl ColorState {
    pub fn new(
        request: ColorRequest,
        capabilities: ColorCapabilities,
        headroom: DisplayHeadroom,
    ) -> Self {
        Self {
            request,
            capabilities,
            headroom,
        }
    }

    pub fn request(self) -> ColorRequest {
        self.request
    }

    pub fn capabilities(self) -> ColorCapabilities {
        self.capabilities
    }

    pub fn headroom(self) -> DisplayHeadroom {
        self.headroom
    }

    pub fn set_request(&mut self, request: ColorRequest) {
        self.request = request;
    }

    pub fn set_capabilities(&mut self, capabilities: ColorCapabilities) {
        self.capabilities = capabilities;
    }

    pub fn set_headroom(&mut self, headroom: DisplayHeadroom) {
        self.headroom = headroom;
    }

    pub fn resolve(self, adapter: &dyn ColorAdapter) -> ResolvedColorPath {
        adapter.resolve(self.request, self.capabilities)
    }
}
