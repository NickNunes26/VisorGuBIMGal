import type { KernelPort } from '../domain/kernel/KernelPort';
import { WasmKernelAdapter } from './kernel/WasmKernelAdapter';
import { AutoRenderer } from './renderer/AutoRenderer';

export const kernel: KernelPort = new WasmKernelAdapter();
export const renderer = new AutoRenderer();
