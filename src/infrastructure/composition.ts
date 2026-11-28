import type { FilePort } from '../domain/files/FilePort';
import type { KernelPort } from '../domain/kernel/KernelPort';
import { DesktopFilePort, isDesktopHost } from './files/DesktopFilePort';
import { WebFilePort } from './files/WebFilePort';
import { WasmKernelAdapter } from './kernel/WasmKernelAdapter';
import { AutoRenderer } from './renderer/AutoRenderer';

export const kernel: KernelPort = new WasmKernelAdapter();
export const renderer = new AutoRenderer();
export const filePort: FilePort = isDesktopHost() ? new DesktopFilePort() : new WebFilePort();
