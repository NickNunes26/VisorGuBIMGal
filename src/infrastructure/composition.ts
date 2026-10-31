import type { KernelPort } from '../domain/kernel/KernelPort';
import type { RendererPort } from '../domain/kernel/RendererPort';
import { HybridKernelAdapter } from './ifclite/HybridKernelAdapter';
import { IfcLiteKernelAdapter } from './ifclite/IfcLiteKernelAdapter';
import { WasmKernelAdapter } from './kernel/WasmKernelAdapter';
import { WebGpuRenderer } from './renderer/WebGpuRenderer';

export const kernel: KernelPort = new HybridKernelAdapter(
  new WasmKernelAdapter(),
  new IfcLiteKernelAdapter()
);
export const renderer: RendererPort = new WebGpuRenderer();

