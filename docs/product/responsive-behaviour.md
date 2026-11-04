# Responsive application behaviour

Limiara uses content-driven layout thresholds rather than device names. The
same routes and metadata remain authoritative at every width; CSS changes their
composition without replacing them with a separate mobile application.

## Layout thresholds

| Available CSS width | Shell and content behaviour |
| --- | --- |
| Above `68rem` | Full module rail, three-part workspace bar, wide form guidance and collection preview. |
| Above `62rem` to `68rem` | Full rail remains and top-bar identity is compact. |
| Above `48rem` to `62rem` | Full rail remains, while collection rows switch to their compact representation, form guidance stacks, and sticky collection previews release. |
| At or below `48rem` | The rail becomes a focus-trapped navigation drawer, global search leaves the top bar, grids and split views become one column, and list rows use their compact card representation. |
| At or below `36rem` | Page titles and primary actions stack so long names and translated action labels cannot squeeze or clip one another. |
| At or below `28rem` | Query fields and their actions stack rather than competing for the same row. |

These are layout boundaries, not supported-device boundaries. A 1440-pixel
desktop viewport at 200% zoom has an effective width of 720 CSS pixels and must
therefore receive the compact shell. The minimum supported reflow width is 320
CSS pixels.

## Interaction invariants

- Routes, query filters, selected records, and unsaved controlled-form values
  survive viewport and orientation changes.
- Primary controls retain a minimum `44px` target, and compact action rows wrap
  labels instead of clipping them.
- Dense collections become compact records at narrow widths; only a bounded
  data table may scroll inside its own labelled container.
- Form actions stop floating in short landscape or virtual-keyboard-sized
  viewports so every field and action remains reachable.
- Long names, email addresses, translated-length labels, empty results,
  validation feedback, and denied/error messages must not introduce page-level
  horizontal scrolling.

## Verification

Run `pnpm test:contacts-visual`. The suite exercises the Contacts directory and
record journeys at 1440 x 900, 834 x 1194, and 390 x 844, retains screenshot
baselines for each, and checks 320-pixel and 200%-zoom-equivalent reflow. It also
changes a populated form between phone, tablet, and short landscape viewports
to prove unsaved values and actions remain available.
