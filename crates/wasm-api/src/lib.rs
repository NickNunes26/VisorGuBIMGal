//! WebAssembly facade of the kernel. Owner: Claude (T-033). Contract: `docs/05_kernel_contract.md`.

use wasm_bindgen::prelude::*;

/// Kernel version, so the app can detect a stale `.wasm` next to newer TypeScript.
#[wasm_bindgen(js_name = kernelVersion)]
pub fn kernel_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_crate_version() {
        assert_eq!(kernel_version(), env!("CARGO_PKG_VERSION"));
    }
}
