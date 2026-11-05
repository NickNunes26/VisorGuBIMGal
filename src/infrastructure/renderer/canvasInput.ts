import type { Camera } from './Camera';

/** Travel in CSS pixels, from where the pointer went down, under which a release still picks. */
const CLICK_SLOP = 3;

export interface CanvasInputHandlers {
  /** The camera moved and the view needs a frame. */
  onViewChange(): void;
  /** A press and release that did not move, in CSS pixels from the canvas's top left. */
  onClick(cssX: number, cssY: number): void;
  /** A mouse moving with no button down, in CSS pixels; null once it leaves the canvas. */
  onHover?(at: { x: number; y: number } | null): void;
  /** The mouse cursor while no button is down; 'grab' if not given. */
  idleCursor?(): string;
}

interface Pair {
  midX: number;
  midY: number;
  separation: number;
}

/**
 * Mouse, pen and touch on the canvas, through pointer events. One pointer orbits, or pans with the
 * middle or right button or Shift; two touches pinch to zoom and move together to pan; a press that
 * does not move picks. Returns a function that removes everything it added.
 */
export function attachCanvasInput(
  canvas: HTMLCanvasElement,
  camera: Camera,
  handlers: CanvasInputHandlers
): () => void {
  /** Pointers down on the canvas, at their last applied position. */
  const pointers = new Map<number, { x: number; y: number }>();
  let panning = false;
  let pressX = 0;
  let pressY = 0;
  let pressButton = 0;
  /** Since the first pointer went down: it travelled, or a second one joined. */
  let moved = false;
  let multi = false;
  /** The first two pointers as last applied; the pair is applied once per frame, so both fingers
   *  of one touch move land together and a two-finger pan does not wobble the zoom. */
  let lastPair: Pair | null = null;
  let pairFrame = 0;

  const pairOf = (): Pair | null => {
    if (pointers.size < 2) return null;
    const [a, b] = pointers.values();
    return {
      midX: (a.x + b.x) / 2,
      midY: (a.y + b.y) / 2,
      separation: Math.hypot(b.x - a.x, b.y - a.y)
    };
  };

  const applyPair = () => {
    pairFrame = 0;
    const pair = pairOf();
    if (!pair || !lastPair) {
      lastPair = pair;
      return;
    }
    if (pair.separation >= 1 && lastPair.separation >= 1) {
      camera.zoomBy(lastPair.separation / pair.separation);
    }
    camera.pan(pair.midX - lastPair.midX, pair.midY - lastPair.midY);
    lastPair = pair;
    handlers.onViewChange();
  };

  /** Applies any pending two-finger move, or drops it, before the set of pointers changes. */
  const settlePair = (apply: boolean) => {
    if (pairFrame) {
      cancelAnimationFrame(pairFrame);
      pairFrame = 0;
      if (apply) applyPair();
    }
  };

  const onPointerDown = (e: PointerEvent) => {
    if (pointers.size === 0) {
      moved = false;
      multi = false;
      pressX = e.clientX;
      pressY = e.clientY;
      pressButton = e.button;
      panning = e.pointerType !== 'touch' && (e.button === 1 || e.button === 2 || e.shiftKey);
      if (e.pointerType === 'mouse') canvas.style.cursor = panning ? 'move' : 'grabbing';
    } else {
      settlePair(true);
      multi = true;
    }
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    lastPair = pairOf();
    try {
      canvas.setPointerCapture(e.pointerId);
    } catch {
      // A pointer that is already gone cannot be captured; its release still arrives.
    }
  };

  const idleCursor = () => handlers.idleCursor?.() ?? 'grab';

  const onPointerMove = (e: PointerEvent) => {
    const pointer = pointers.get(e.pointerId);
    if (!pointer) {
      if (e.pointerType === 'mouse' && pointers.size === 0 && handlers.onHover) {
        const rect = canvas.getBoundingClientRect();
        handlers.onHover({ x: e.clientX - rect.left, y: e.clientY - rect.top });
      }
      return;
    }
    if (Math.hypot(e.clientX - pressX, e.clientY - pressY) > CLICK_SLOP) moved = true;

    if (pointers.size >= 2) {
      pointer.x = e.clientX;
      pointer.y = e.clientY;
      if (!pairFrame) pairFrame = requestAnimationFrame(applyPair);
      return;
    }

    const dx = e.clientX - pointer.x;
    const dy = e.clientY - pointer.y;
    pointer.x = e.clientX;
    pointer.y = e.clientY;
    if (panning) {
      camera.pan(dx, dy);
    } else {
      camera.orbit(dx, dy);
    }
    handlers.onViewChange();
  };

  const release = (e: PointerEvent, completed: boolean) => {
    if (!pointers.has(e.pointerId)) return;
    settlePair(completed);
    const wasOnly = pointers.size === 1;
    pointers.delete(e.pointerId);
    lastPair = pairOf();
    if (pointers.size > 0) return;

    panning = false;
    if (e.pointerType === 'mouse') canvas.style.cursor = idleCursor();
    if (completed && wasOnly && !moved && !multi && pressButton === 0) {
      const rect = canvas.getBoundingClientRect();
      handlers.onClick(e.clientX - rect.left, e.clientY - rect.top);
    }
  };

  const onPointerUp = (e: PointerEvent) => release(e, true);
  const onPointerCancel = (e: PointerEvent) => release(e, false);

  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    camera.zoom(e.deltaY);
    handlers.onViewChange();
  };

  const onContextMenu = (e: MouseEvent) => {
    e.preventDefault();
  };

  const onPointerLeave = (e: PointerEvent) => {
    if (e.pointerType === 'mouse' && pointers.size === 0) handlers.onHover?.(null);
  };

  canvas.addEventListener('pointerdown', onPointerDown);
  canvas.addEventListener('pointermove', onPointerMove);
  canvas.addEventListener('pointerup', onPointerUp);
  canvas.addEventListener('pointercancel', onPointerCancel);
  canvas.addEventListener('lostpointercapture', onPointerCancel);
  canvas.addEventListener('wheel', onWheel, { passive: false });
  canvas.addEventListener('contextmenu', onContextMenu);
  canvas.addEventListener('pointerleave', onPointerLeave);
  // The browser must not scroll or zoom the page under a finger on the canvas.
  canvas.style.touchAction = 'none';
  canvas.style.cursor = idleCursor();

  return () => {
    settlePair(false);
    canvas.removeEventListener('pointerdown', onPointerDown);
    canvas.removeEventListener('pointermove', onPointerMove);
    canvas.removeEventListener('pointerup', onPointerUp);
    canvas.removeEventListener('pointercancel', onPointerCancel);
    canvas.removeEventListener('lostpointercapture', onPointerCancel);
    canvas.removeEventListener('wheel', onWheel);
    canvas.removeEventListener('contextmenu', onContextMenu);
    canvas.removeEventListener('pointerleave', onPointerLeave);
  };
}
