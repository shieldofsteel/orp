import '@testing-library/jest-dom';
import { vi } from 'vitest';

// ── Canvas stub for jsdom ──────────────────────────────────────────────────────
// jsdom does not implement HTMLCanvasElement.prototype.getContext.
// Components that draw to canvas (MiniMap, SpeedGraph in EntityInspector) and
// Leaflet's internal canvas renderer will throw "Not implemented" without this.
// The returned 2D context implements the small subset our code touches —
// extend if a test asserts on a specific canvas command.
const canvas2DStub: Partial<CanvasRenderingContext2D> = {
  fillRect: vi.fn(),
  clearRect: vi.fn(),
  strokeRect: vi.fn(),
  beginPath: vi.fn(),
  closePath: vi.fn(),
  moveTo: vi.fn(),
  lineTo: vi.fn(),
  arc: vi.fn(),
  stroke: vi.fn(),
  fill: vi.fn(),
  fillText: vi.fn(),
  strokeText: vi.fn(),
  measureText: vi.fn(() => ({ width: 0 } as TextMetrics)),
  setTransform: vi.fn(),
  save: vi.fn(),
  restore: vi.fn(),
  translate: vi.fn(),
  scale: vi.fn(),
  drawImage: vi.fn(),
  getImageData: vi.fn(() => ({ data: new Uint8ClampedArray(4) } as ImageData)),
  putImageData: vi.fn(),
  createImageData: vi.fn(() => ({ data: new Uint8ClampedArray(4) } as ImageData)),
};
HTMLCanvasElement.prototype.getContext = vi.fn(
  () => canvas2DStub as CanvasRenderingContext2D,
) as unknown as HTMLCanvasElement['getContext'];

// Leaflet calls these during tile-layer init.
HTMLCanvasElement.prototype.toDataURL = vi.fn(() => 'data:image/png;base64,');
window.URL.createObjectURL = vi.fn(() => 'blob:');
