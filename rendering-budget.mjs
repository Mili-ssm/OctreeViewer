export const MAX_FRAMEBUFFER_PIXELS = 4_194_304;
export const MAX_FRAMEBUFFER_DIMENSION = 4_096;
export const MAX_DEVICE_PIXEL_RATIO = 2;

export function framebufferSize(clientWidth, clientHeight, devicePixelRatio = 1) {
    const width = Math.max(0, Number(clientWidth) || 0);
    const height = Math.max(0, Number(clientHeight) || 0);
    if (width === 0 || height === 0) return [0, 0];

    const requestedRatio = Math.min(MAX_DEVICE_PIXEL_RATIO, Math.max(0.5, Number(devicePixelRatio) || 1));
    const requestedWidth = width * requestedRatio;
    const requestedHeight = height * requestedRatio;
    const pixelScale = Math.sqrt(
        MAX_FRAMEBUFFER_PIXELS / (requestedWidth * requestedHeight),
    );
    const dimensionScale = Math.min(
        MAX_FRAMEBUFFER_DIMENSION / requestedWidth,
        MAX_FRAMEBUFFER_DIMENSION / requestedHeight,
    );
    const scale = Math.min(1, pixelScale, dimensionScale);

    return [
        Math.max(1, Math.floor(requestedWidth * scale)),
        Math.max(1, Math.floor(requestedHeight * scale)),
    ];
}
