# Color key and filter redesign

The design for merging "Color By" and the bar's Filter tab into one color key,
where every color of every category is a toggle that hides the systems drawn
in it. [`SPEC.md`](./SPEC.md) is the written spec. The mockups below are
treated as authoritative alongside it: where the two disagree, ask.

`mockups/` holds each artboard rendered to PNG. `source/` holds the canvas the
artboards were drawn in (`.dc.html`, one per artboard, with `canvas.json` as
its index), which renders only inside the Design canvas runtime; the PNGs are
what to read.

## Elements

| | |
|---|---|
| Allegiance key: Other collapsed, and expanded with Independent hidden | ![](mockups/KeyAllegiance.png) |
| Government key: one group per hue | ![](mockups/KeyGovernment.png) |
| The whole Filter tab: key, footer, faction lookup, recency | ![](mockups/FilterTab.png) |
| Color row: all shown, Other hidden, Federation solo, mask lifted | ![](mockups/BarRows.png) |
| Mini legend: popover on hover, and bare above the rose | ![](mockups/KeyCollapsed.png) |

## Full view

The interactive board, at its default state (the map idle, the color row
under the bar).

![](mockups/Main.png)

## Flows

**A · Glance**

1. Idle map, color row under the bar ![](mockups/Flow-A1.png)
2. Hover the row: the mini legend names the chips ![](mockups/Flow-A2.png)

**B · Quick toggle**

1. Click the Other chip: all 7 hidden ![](mockups/Flow-B1.png)
2. Alt-click Federation: solo ![](mockups/Flow-B2.png)

**C · Full edit**

1. Click ALLEGIANCE: the Filter tab opens on the key ![](mockups/Flow-C1.png)
2. Expand Other, hide Independent ![](mockups/Flow-C2.png)
3. Switch to GOVERNMENT, hide Prison and Prison Colony ![](mockups/Flow-C3.png)
4. Close the form: the row reads `2 hidden` (the mockup's `+1` is from the first
   draft, where other categories also applied) ![](mockups/Flow-C4.png)

**D · Lift the mask**

1. Uncheck the row: everything comes back, the summary is struck
   ![](mockups/Flow-D1.png)

**E · Interface hidden**

1. The mini legend stands above the rose ![](mockups/Flow-E1.png)
