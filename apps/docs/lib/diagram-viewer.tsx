'use client';

import { useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react';

type Diagram = { svg: string; width: number };
const levels = [0.75, 1, 1.25, 1.5, 2, 2.5, 3];
const initialLevel = 1;
const panKeys: Record<string, [number, number]> = {
  ArrowLeft: [-48, 0],
  ArrowRight: [48, 0],
  ArrowUp: [0, -48],
  ArrowDown: [0, 48],
};

export function DiagramViewer({ label, wide, narrow }: { label: string; wide: Diagram; narrow?: Diagram }) {
  const title = useId();
  const help = useId();
  const figure = useRef<HTMLElement>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const expand = useRef<HTMLButtonElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);
  const drag = useRef<{ id: number; x: number; y: number } | null>(null);
  const center = useRef<{ x: number; y: number } | null>(null);
  const [level, setLevel] = useState(initialLevel);
  const [inlineHeight, setInlineHeight] = useState(0);
  const scale = levels[level];
  const expanded = inlineHeight > 0;

  useLayoutEffect(() => {
    if (expanded) {
      if (!dialog.current?.open) dialog.current?.showModal();
      close.current?.focus();
    } else if (restoreFocus.current) {
      expand.current?.focus({ preventScroll: true });
      restoreFocus.current = false;
    }
  }, [expanded]);

  useLayoutEffect(() => {
    const box = viewport.current;
    if (!box || !center.current) return;
    const { x, y } = center.current;
    box.scrollTo({
      left: x * box.scrollWidth - box.clientWidth / 2,
      top: y * box.scrollHeight - box.clientHeight / 2,
    });
    center.current = null;
  }, [level]);

  function zoom(next: number, reset = false) {
    const box = viewport.current;
    if (!box || next < 0 || next >= levels.length) return;
    if (reset) {
      center.current = null;
      box.scrollTo(0, 0);
    } else if (next !== level) {
      center.current = {
        x: (box.scrollLeft + box.clientWidth / 2) / box.scrollWidth,
        y: (box.scrollTop + box.clientHeight / 2) / box.scrollHeight,
      };
    }
    setLevel(next);
  }

  function handleKey(event: KeyboardEvent<HTMLDivElement>) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const offset = panKeys[event.key];
    if (offset) event.currentTarget.scrollBy(...offset);
    else if (event.key === '+' || event.key === '=') zoom(level + 1);
    else if (event.key === '-') zoom(level - 1);
    else if (event.key === '0') zoom(initialLevel, true);
    else return;
    event.preventDefault();
  }

  const viewer = (
    <>
      <div className="diagram-toolbar">
        <span id={title}>{label}</span>
        <div className="diagram-controls">
          <button type="button" title="Zoom out" aria-label="Zoom out" aria-disabled={level === 0} onClick={() => zoom(level - 1)}>−</button>
          <output aria-label="Diagram zoom">{Math.round(scale * 100)}%</output>
          <button type="button" title="Zoom in" aria-label="Zoom in" aria-disabled={level === levels.length - 1} onClick={() => zoom(level + 1)}>+</button>
          <button type="button" title="Reset zoom" onClick={() => zoom(initialLevel, true)}>Reset</button>
          {expanded
            ? <button type="button" ref={close} onClick={() => dialog.current?.close()}>Close</button>
            : <button type="button" ref={expand} aria-haspopup="dialog" onClick={() => setInlineHeight(figure.current?.offsetHeight ?? 0)}>Expand</button>}
        </div>
      </div>
      <div
        ref={viewport}
        className="diagram-viewport"
        role="region"
        aria-label={`${label} canvas`}
        aria-describedby={help}
        tabIndex={0}
        onKeyDown={handleKey}
        onPointerDown={(event) => {
          const box = event.currentTarget;
          if (event.pointerType === 'touch' || event.button !== 0 ||
            (box.scrollWidth <= box.clientWidth && box.scrollHeight <= box.clientHeight)) return;
          drag.current = { id: event.pointerId, x: event.clientX, y: event.clientY };
          box.setPointerCapture(event.pointerId);
          box.dataset.dragging = 'true';
        }}
        onPointerMove={(event) => {
          const point = drag.current;
          if (point?.id !== event.pointerId) return;
          event.currentTarget.scrollBy(point.x - event.clientX, point.y - event.clientY);
          point.x = event.clientX;
          point.y = event.clientY;
        }}
        onLostPointerCapture={(event) => {
          drag.current = null;
          delete event.currentTarget.dataset.dragging;
        }}
        style={{
          '--diagram-width': `${wide.width * scale}px`,
          '--diagram-narrow-width': narrow ? `${narrow.width * scale}px` : undefined,
        } as CSSProperties}
      >
        <div className="diagram-stage">
          <div className="diagram-svg" dangerouslySetInnerHTML={{ __html: wide.svg }} />
          {narrow && <div className="diagram-svg diagram-svg-narrow" dangerouslySetInnerHTML={{ __html: narrow.svg }} />}
        </div>
      </div>
      <p className="diagram-hint" id={help}>Drag or swipe to pan. With the canvas focused, use arrow keys to pan, + or − to zoom, and 0 to reset.</p>
    </>
  );

  return (
    <figure className="diagram-figure" ref={figure} style={expanded ? { height: inlineHeight } : undefined}>
      {!expanded && viewer}
      <dialog
        ref={dialog}
        className="diagram-dialog"
        aria-labelledby={title}
        onClose={() => {
          restoreFocus.current = true;
          setInlineHeight(0);
        }}
      >
        {expanded && viewer}
      </dialog>
    </figure>
  );
}
