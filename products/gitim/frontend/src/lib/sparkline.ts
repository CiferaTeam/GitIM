export function sparklinePath(values: number[], w: number, h: number): string {
  if (values.length === 0) return "";
  if (values.length === 1) {
    const y = h / 2;
    return `M0,${y.toFixed(1)} L${w.toFixed(1)},${y.toFixed(1)}`;
  }
  const max = Math.max(1, ...values);
  const step = w / (values.length - 1);
  return values
    .map((v, i) => {
      const x = i * step;
      const y = h - (v / max) * h;
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}
