// Single source of truth for the Blaze brand mark: the BlazeAPI flame, as in
// the app icon. Stroke uses the given colour (currentColor by default).

const MARK_PATH =
  'M8.5 14.5A2.5 2.5 0 0 0 11 12c0-1.38-.5-2-1-3-1.072-2.143-.224-4.054 2-6 .5 2.5 2 4.9 4 6.5 2 1.6 3 3.5 3 5.5a7 7 0 1 1-14 0c0-1.153.433-2.294 1-3a2.5 2.5 0 0 0 2.5 2.5z';

/** Brand mark as an inline SVG string. `size` is width/height in px. */
export function brandMarkSvg(size = 28, stroke = 'currentColor', strokeWidth = 2): string {
  return (
    `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" ` +
    `stroke="${stroke}" stroke-width="${strokeWidth}" stroke-linecap="round" stroke-linejoin="round">` +
    `<path d="${MARK_PATH}"/></svg>`
  );
}

/** The flame in BlazeAPI orange, for the splash and sign-in screens. */
export const BRAND_ORANGE = '#ff8a2a';
