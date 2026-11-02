import type { KernelPort } from '../domain/kernel/KernelPort';
import type { RendererPort } from '../domain/kernel/RendererPort';
import { WasmKernelAdapter } from './kernel/WasmKernelAdapter';
import { AutoRenderer } from './renderer/AutoRenderer';

export const kernel: KernelPort = new WasmKernelAdapter();
export const renderer: RendererPort = new AutoRenderer();
