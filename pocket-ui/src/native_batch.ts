export type NativeImageHandleCommand = { type: "image_handle"; handle: number; x: number; y: number; width: number; height: number };

export function emitImageHandle(handle: number, x: number, y: number, width: number, height: number): boolean {
  if (!Number.isSafeInteger(handle) || handle <= 0 || handle > 0xffff_ffff
    || ![x, y, width, height].every(Number.isFinite)
    || x < 0 || y < 0 || x >= 480 || y >= 272
    || width <= 0 || height <= 0 || x + width > 480 || y + height > 272) return false;
  if (globalThis.nativeBatch.commands.length >= 8) return false;
  globalThis.nativeBatch.commands.push({ type: "image_handle", handle, x, y, width, height });
  return true;
}
