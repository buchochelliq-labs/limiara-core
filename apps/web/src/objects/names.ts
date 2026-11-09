/** Turns a label into a short API name: `Project stage` → `project_stage`. */
export function suggestName(label: string): string {
  const name = label.toLowerCase().normalize('NFKD').replace(/[^a-z0-9]+/g, '_').replace(/^[^a-z]+/, '').replace(/_+$/, '').slice(0, 63);
  return name.replace(/_+$/, '');
}
