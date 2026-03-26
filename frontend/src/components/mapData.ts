// Simplified coastline outlines for spatial reference (no base map needed).
// These are rough polygonal outlines — enough to orient users near Western Europe.

/** Simplified European coastline segments (lon, lat pairs) */
export const EUROPE_COASTLINE: [number, number][][] = [
  // UK South coast + English Channel
  [
    [-5.7, 50.1], [-4.8, 50.3], [-3.5, 50.4], [-1.8, 50.7], [-1.1, 50.8],
    [0.3, 51.0], [1.0, 51.1], [1.4, 51.4], [1.6, 52.0], [1.7, 52.6],
    [0.5, 53.0], [-0.2, 53.6], [-0.4, 54.0], [-1.2, 54.6], [-3.0, 54.8],
    [-3.4, 54.9], [-4.8, 55.0], [-5.0, 55.3],
  ],
  // Netherlands, Belgium, NW Germany coast
  [
    [1.6, 51.0], [2.5, 51.1], [3.3, 51.4], [3.6, 51.5], [4.0, 51.9],
    [4.3, 52.0], [4.8, 52.5], [5.0, 53.0], [5.4, 53.2], [5.8, 53.4],
    [6.2, 53.5], [6.9, 53.6], [7.2, 53.6], [8.0, 53.9], [8.5, 54.0],
    [8.8, 54.3], [8.6, 54.8], [9.0, 54.8],
  ],
  // France Atlantic coast
  [
    [-1.8, 46.2], [-1.3, 46.3], [-1.2, 46.7], [-1.6, 47.0], [-2.5, 47.3],
    [-2.8, 47.5], [-3.0, 47.6], [-4.0, 47.9], [-4.5, 48.4], [-4.8, 48.5],
    [-3.9, 48.7], [-3.0, 48.8], [-2.0, 48.6], [-1.6, 48.6], [-1.2, 48.8],
    [-1.0, 49.2], [-1.2, 49.7], [0.2, 49.5], [1.0, 50.0], [1.6, 51.0],
  ],
  // Norway SW coast
  [
    [5.0, 58.0], [5.5, 58.5], [5.3, 59.0], [5.5, 59.5], [5.2, 60.0],
    [5.0, 60.5], [5.3, 61.0], [5.0, 61.5], [5.5, 62.0],
  ],
  // Denmark
  [
    [8.1, 54.8], [8.2, 55.0], [8.6, 55.5], [8.3, 56.0], [8.1, 56.5],
    [8.6, 57.0], [9.5, 57.0], [10.0, 57.4], [10.5, 57.7], [10.0, 56.5],
    [10.3, 56.0], [10.8, 55.8], [12.0, 55.6], [12.5, 55.9], [12.6, 56.0],
  ],
];

/** Coordinate grid lines covering the main area of interest (Western Europe / North Sea) */
export function generateGrid(
  lonMin: number,
  lonMax: number,
  latMin: number,
  latMax: number,
  step: number
): { path: [number, number][]; label?: string }[] {
  const lines: { path: [number, number][]; label?: string }[] = [];

  // Longitude lines (vertical)
  for (let lon = Math.ceil(lonMin / step) * step; lon <= lonMax; lon += step) {
    lines.push({
      path: [
        [lon, latMin],
        [lon, latMax],
      ],
      label: `${lon}°`,
    });
  }

  // Latitude lines (horizontal)
  for (let lat = Math.ceil(latMin / step) * step; lat <= latMax; lat += step) {
    lines.push({
      path: [
        [lonMin, lat],
        [lonMax, lat],
      ],
      label: `${lat}°`,
    });
  }

  return lines;
}
