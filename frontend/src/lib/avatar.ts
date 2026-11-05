/** Client-side avatar preparation (Phase 9). */

export const AVATAR_MAX_BYTES = 2 * 1024 * 1024; // 2 MB, mirrors the server
export const AVATAR_ACCEPTED = ["image/jpeg", "image/png", "image/webp"] as const;
export const AVATAR_OUTPUT_SIZE = 512;

export class AvatarError extends Error {}

/**
 * Validate an uploaded image and center-crop it to a square, re-encoded at
 * `AVATAR_OUTPUT_SIZE`×`AVATAR_OUTPUT_SIZE` as a data URL. WebP is preferred
 * (small payloads); PNG is the fallback where the browser cannot encode WebP.
 * The server independently validates format, size, and squareness.
 */
export async function fileToAvatarDataUrl(file: File): Promise<string> {
  if (!AVATAR_ACCEPTED.includes(file.type as (typeof AVATAR_ACCEPTED)[number])) {
    throw new AvatarError("Choose a JPG, PNG or WebP image.");
  }
  if (file.size > AVATAR_MAX_BYTES) {
    throw new AvatarError("Images must be 2 MB or smaller.");
  }
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new AvatarError("That file could not be read as an image.");
  }
  try {
    const side = Math.min(bitmap.width, bitmap.height);
    const canvas = document.createElement("canvas");
    canvas.width = AVATAR_OUTPUT_SIZE;
    canvas.height = AVATAR_OUTPUT_SIZE;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new AvatarError("Your browser cannot process images.");
    ctx.drawImage(
      bitmap,
      (bitmap.width - side) / 2,
      (bitmap.height - side) / 2,
      side,
      side,
      0,
      0,
      AVATAR_OUTPUT_SIZE,
      AVATAR_OUTPUT_SIZE,
    );
    const webp = canvas.toDataURL("image/webp", 0.9);
    if (webp.startsWith("data:image/webp")) return webp;
    const png = canvas.toDataURL("image/png");
    if (png.length > AVATAR_MAX_BYTES) {
      throw new AvatarError("The cropped image is too large; try a smaller file.");
    }
    return png;
  } finally {
    bitmap.close();
  }
}
