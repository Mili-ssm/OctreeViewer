// Empty-pixel measure for the render benchmark (render plan R0, E3). Node-side and opt-in: the probe
// takes a WebDriver BiDi screenshot of the canvas, decodes it here and counts background pixels
// inside the projected dataset bounds. Nothing here runs in the viewer.
import zlib from "node:zlib";

// RendererConfig::default clear colour (0.04, 0.06, 0.08) as 8-bit channels.
export const BACKGROUND_RGB = [10, 15, 20];
const VERTICAL_FOV = 45 * Math.PI / 180;
const NEAR = 1e-3;

/** Decodes a non-interlaced 8-bit RGB/RGBA PNG into `{width, height, rgba}`. */
export function decodePng(buffer) {
    if (buffer.readUInt32BE(0) !== 0x89504e47) throw new Error("not a PNG");
    let offset = 8;
    let header = null;
    const data = [];
    while (offset < buffer.length) {
        const length = buffer.readUInt32BE(offset);
        const type = buffer.toString("latin1", offset + 4, offset + 8);
        const body = buffer.subarray(offset + 8, offset + 8 + length);
        if (type === "IHDR") {
            header = { width: body.readUInt32BE(0), height: body.readUInt32BE(4), depth: body[8], color: body[9], interlace: body[12] };
        } else if (type === "IDAT") data.push(body);
        else if (type === "IEND") break;
        offset += 12 + length;
    }
    if (!header || header.depth !== 8 || header.interlace !== 0 || (header.color !== 2 && header.color !== 6)) {
        throw new Error("unsupported PNG (need 8-bit RGB/RGBA, non-interlaced)");
    }
    const channels = header.color === 6 ? 4 : 3;
    const stride = header.width * channels;
    const raw = zlib.inflateSync(Buffer.concat(data));
    const pixels = Buffer.alloc(stride * header.height);
    for (let row = 0; row < header.height; row++) {
        const filter = raw[row * (stride + 1)];
        const src = row * (stride + 1) + 1;
        for (let i = 0; i < stride; i++) {
            const left = i >= channels ? pixels[row * stride + i - channels] : 0;
            const up = row > 0 ? pixels[(row - 1) * stride + i] : 0;
            const upLeft = row > 0 && i >= channels ? pixels[(row - 1) * stride + i - channels] : 0;
            let predictor = 0;
            if (filter === 1) predictor = left;
            else if (filter === 2) predictor = up;
            else if (filter === 3) predictor = (left + up) >> 1;
            else if (filter === 4) {
                const p = left + up - upLeft;
                const pa = Math.abs(p - left), pb = Math.abs(p - up), pc = Math.abs(p - upLeft);
                predictor = pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
            } else if (filter !== 0) throw new Error(`bad PNG filter ${filter}`);
            pixels[row * stride + i] = (raw[src + i] + predictor) & 0xff;
        }
    }
    const rgba = new Uint8Array(header.width * header.height * 4);
    for (let i = 0; i < header.width * header.height; i++) {
        rgba[i * 4] = pixels[i * channels];
        rgba[i * 4 + 1] = pixels[i * channels + 1];
        rgba[i * 4 + 2] = pixels[i * channels + 2];
        rgba[i * 4 + 3] = channels === 4 ? pixels[i * channels + 3] : 255;
    }
    return { width: header.width, height: header.height, rgba };
}

const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const unit = (a) => { const n = Math.hypot(...a); return [a[0] / n, a[1] / n, a[2] / n]; };

/**
 * Projects the 8 AABB corners with the viewer's fixed 45 degree vertical FOV and +Z up.
 * Returns pixel coordinates, or null when a corner is behind the camera (hull would be unbounded).
 */
export function projectBounds(position, target, boundsMin, boundsMax, width, height) {
    const forward = unit(sub(target, position));
    const right = unit(cross(forward, [0, 0, 1]));
    const up = cross(right, forward);
    const tan = Math.tan(VERTICAL_FOV / 2);
    const points = [];
    for (let corner = 0; corner < 8; corner++) {
        const offset = sub([
            corner & 1 ? boundsMax[0] : boundsMin[0],
            corner & 2 ? boundsMax[1] : boundsMin[1],
            corner & 4 ? boundsMax[2] : boundsMin[2],
        ], position);
        const depth = dot(offset, forward);
        if (depth <= NEAR) return null;
        points.push([
            (dot(offset, right) / (depth * tan * (width / height)) + 1) / 2 * width,
            (1 - dot(offset, up) / (depth * tan)) / 2 * height,
        ]);
    }
    return points;
}

/** Andrew's monotone chain; counter-clockwise in a y-up frame (clockwise on screen). */
export function convexHull(points) {
    const sorted = [...points].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    const turn = (o, a, b) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    const build = (list) => {
        const hull = [];
        for (const point of list) {
            while (hull.length >= 2 && turn(hull.at(-2), hull.at(-1), point) <= 0) hull.pop();
            hull.push(point);
        }
        hull.pop();
        return hull;
    };
    return [...build(sorted), ...build(sorted.reverse())];
}

/**
 * Counts pixels inside `hull` (clipped to the image) and how many of them match the background.
 * Without a hull (camera inside the bounds) the whole image counts as the region.
 */
export function countEmptyPixels({ width, height, rgba }, hull = null, background = BACKGROUND_RGB, tolerance = 4) {
    let region = 0;
    let empty = 0;
    for (let y = 0; y < height; y++) {
        let from = 0;
        let to = width - 1;
        if (hull) {
            // Intersect the scanline (pixel centre) with the convex polygon.
            const cy = y + 0.5;
            let low = Infinity;
            let high = -Infinity;
            for (let i = 0; i < hull.length; i++) {
                const [x1, y1] = hull[i];
                const [x2, y2] = hull[(i + 1) % hull.length];
                if ((y1 <= cy && y2 > cy) || (y2 <= cy && y1 > cy)) {
                    const x = x1 + (cy - y1) / (y2 - y1) * (x2 - x1);
                    low = Math.min(low, x);
                    high = Math.max(high, x);
                }
            }
            if (low > high) continue;
            from = Math.max(0, Math.ceil(low - 0.5));
            to = Math.min(width - 1, Math.floor(high - 0.5));
        }
        for (let x = from; x <= to; x++) {
            const i = (y * width + x) * 4;
            region++;
            if (Math.abs(rgba[i] - background[0]) <= tolerance
                && Math.abs(rgba[i + 1] - background[1]) <= tolerance
                && Math.abs(rgba[i + 2] - background[2]) <= tolerance) empty++;
        }
    }
    return { regionPixels: region, emptyPixels: empty, emptyFraction: region ? empty / region : null };
}
