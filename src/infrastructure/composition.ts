import type { KernelPort } from '../domain/kernel/KernelPort';
import type { RendererPort } from '../domain/kernel/RendererPort';
import { IfcLiteKernelAdapter } from './ifclite/IfcLiteKernelAdapter';
import { IfcLiteRendererAdapter } from './ifclite/IfcLiteRendererAdapter';

export const kernel: KernelPort = new IfcLiteKernelAdapter();
export const renderer: RendererPort = new IfcLiteRendererAdapter();
