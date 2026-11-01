//! WebAssembly facade of the kernel. Owner: Claude (T-033). Contract: `docs/05_kernel_contract.md`.

pub mod batches;
mod encode;
mod json;
pub mod loaded;

use loaded::{LoadedModel, OpenError};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::prelude::*;

static ALLOCATION_FAILED: AtomicBool = AtomicBool::new(false);

/// Remembers a failed allocation. Rust then aborts, which in WASM is a bare `unreachable` trap
/// that JS cannot tell from any other bug.
struct TrackingAllocator;

impl TrackingAllocator {
    fn track(pointer: *mut u8) -> *mut u8 {
        if pointer.is_null() {
            ALLOCATION_FAILED.store(true, Ordering::Relaxed);
        }
        pointer
    }
}

// SAFETY: every call forwards to `System` with the caller's own arguments.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        Self::track(unsafe { System.alloc(layout) })
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract.
        Self::track(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
        Self::track(unsafe { System.realloc(pointer, layout, size) })
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

/// Kernel version, so the app can detect a stale `.wasm` next to newer TypeScript.
#[wasm_bindgen(js_name = kernelVersion)]
pub fn kernel_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Whether memory ran out. Safe to call after a trap: it touches no `Model`.
#[wasm_bindgen(js_name = allocationFailed)]
pub fn allocation_failed() -> bool {
    ALLOCATION_FAILED.load(Ordering::Relaxed)
}

/// Encoded as `code|offset|message`, which the Worker turns into a `KernelError`.
fn open_error(error: OpenError) -> JsError {
    match error {
        OpenError::Parse(error) => JsError::new(&format!("parse|{}|{error}", error.offset())),
        OpenError::UnsupportedSchema(identifiers) => JsError::new(&format!(
            "unsupported-schema||schema not supported: {}",
            identifiers.join(", ")
        )),
    }
}

#[wasm_bindgen]
pub struct Model {
    inner: LoadedModel,
}

#[wasm_bindgen]
impl Model {
    #[wasm_bindgen(constructor)]
    pub fn open(bytes: Vec<u8>) -> Result<Model, JsError> {
        LoadedModel::open(bytes).map(|inner| Model { inner }).map_err(open_error)
    }

    #[wasm_bindgen(js_name = summaryJson)]
    pub fn summary_json(&self) -> String {
        self.inner.summary_json()
    }

    #[wasm_bindgen(js_name = spatialTreeJson)]
    pub fn spatial_tree_json(&self) -> Option<String> {
        self.inner.spatial_tree_json()
    }

    #[wasm_bindgen(js_name = elementDetailsJson)]
    pub fn element_details_json(&self, express_id: u32) -> Option<String> {
        self.inner.element_details_json(express_id)
    }

    #[wasm_bindgen(js_name = startStream)]
    pub fn start_stream(&mut self) {
        self.inner.start_stream();
    }

    /// The Worker pulls the next batch only once it can send it.
    #[wasm_bindgen(js_name = nextBatch)]
    pub fn next_batch(&mut self, max_triangles: u32) -> Option<BatchView> {
        self.inner
            .next_batch(max_triangles as usize)
            .map(|batch| BatchView { batch })
    }

    #[wasm_bindgen(js_name = processedElements)]
    pub fn processed_elements(&self) -> u32 {
        self.inner.processed_elements() as u32
    }

    #[wasm_bindgen(js_name = totalElements)]
    pub fn total_elements(&self) -> u32 {
        self.inner.total_elements() as u32
    }

    #[wasm_bindgen(js_name = finishStream)]
    pub fn finish_stream(&mut self) -> String {
        self.inner.finish_stream()
    }
}

/// Each `take` copies one array out of WASM memory into a buffer JS owns, ready to transfer.
#[wasm_bindgen]
pub struct BatchView {
    batch: batches::Batch,
}

#[wasm_bindgen]
impl BatchView {
    pub fn origin(&self) -> Vec<f64> {
        self.batch.origin.to_vec()
    }

    #[wasm_bindgen(js_name = takePositions)]
    pub fn take_positions(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.batch.positions)
    }

    #[wasm_bindgen(js_name = takeNormals)]
    pub fn take_normals(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.batch.normals)
    }

    #[wasm_bindgen(js_name = takeIndices)]
    pub fn take_indices(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.batch.indices)
    }

    #[wasm_bindgen(js_name = takeMeshTable)]
    pub fn take_mesh_table(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.batch.mesh_table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_crate_version() {
        assert_eq!(kernel_version(), env!("CARGO_PKG_VERSION"));
    }
}
