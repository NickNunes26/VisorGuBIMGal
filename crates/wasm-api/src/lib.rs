//! WebAssembly facade of the kernel. Owner: Claude (T-033). Contract: `docs/05_kernel_contract.md`.

pub mod batches;
mod encode;
mod json;
pub mod loaded;

use loaded::{LoadedModel, OpenError};
use wasm_bindgen::prelude::*;

/// Kernel version, so the app can detect a stale `.wasm` next to newer TypeScript.
#[wasm_bindgen(js_name = kernelVersion)]
pub fn kernel_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
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
