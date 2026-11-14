// R3-Q34: after a GPU-process crash the old OffscreenCanvas no longer presents (Chrome keeps showing
// an empty canvas even though the new device draws), so a recovery needs a fresh canvas element.
// Replaces `canvas` in the DOM by a clone (same id, classes, attributes, inline style and size; the
// transferred context is not copied) and transfers the clone's control. Returns null when the element
// is not in the document: the caller then recovers on the old OffscreenCanvas.
export function replaceCanvasElement(canvas) {
    if (!canvas.isConnected || !canvas.parentNode) return null;
    const replacement = canvas.cloneNode(false);
    replacement.width = canvas.width;
    replacement.height = canvas.height;
    canvas.replaceWith(replacement);
    return { canvas: replacement, offscreen: replacement.transferControlToOffscreen() };
}
