// All UI icons as inline SVG strings. Stroke-based, currentColor only.
// 16x16 default viewBox; sized via CSS or the size attr helpers below.

const svg = (inner: string, viewBox = '0 0 16 16'): string =>
  `<svg viewBox="${viewBox}" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" width="16" height="16">${inner}</svg>`;

export const iconFile = svg(
  '<path d="M4 1.5h5l3 3V14a.5.5 0 0 1-.5.5h-7.5A.5.5 0 0 1 3.5 14V2a.5.5 0 0 1 .5-.5z"/><path d="M9 1.5V5h3.5"/>'
);

export const iconFolder = svg(
  '<path d="M1.5 3.5A.5.5 0 0 1 2 3h4l1.5 2H14a.5.5 0 0 1 .5.5V12a.5.5 0 0 1-.5.5H2a.5.5 0 0 1-.5-.5V3.5z"/>'
);

export const iconFolderOpen = svg(
  '<path d="M1.5 3.5A.5.5 0 0 1 2 3h4l1.5 2h6a.5.5 0 0 1 .5.5V7"/><path d="M3 12.5h11l1.2-5H4.2L3 12.5z"/><path d="M1.5 3.5V12a.5.5 0 0 0 .5.5h1"/>'
);

// New-file: a document with a small "+" in the lower area.
export const iconFilePlus = svg(
  '<path d="M4 1.5h5l3 3V9"/><path d="M9 1.5V5h3.5"/><path d="M4 1.5A.5.5 0 0 0 3.5 2v12a.5.5 0 0 0 .5.5h3"/><path d="M11 11v4M9 13h4"/>'
);

// New-folder: a folder with a small "+" inside.
export const iconFolderPlus = svg(
  '<path d="M1.5 3.5A.5.5 0 0 1 2 3h4l1.5 2H14a.5.5 0 0 1 .5.5V12a.5.5 0 0 1-.5.5H2a.5.5 0 0 1-.5-.5V3.5z"/><path d="M8 7.5v4M6 9.5h4"/>'
);

export const iconChevronRight = svg('<path d="M6 3.5 10.5 8 6 12.5"/>');
export const iconChevronDown = svg('<path d="M3.5 6 8 10.5 12.5 6"/>');
export const iconClose = svg('<path d="M4 4l8 8M12 4l-8 8"/>');
export const iconPlus = svg('<path d="M8 3v10M3 8h10"/>');

export const iconSearch = svg(
  '<circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5 14 14"/>'
);

// Sliders style settings icon
export const iconSettings = svg(
  '<path d="M2 4.5h12M2 11.5h12"/><circle cx="6" cy="4.5" r="1.8"/><circle cx="10" cy="11.5" r="1.8"/>'
);

// Gear / cog settings icon — toothed rim outline + center ring, stroked like
// every other icon (transparent center, so it works over any background).
export const iconGear = svg(
  '<path d="M6.7 1.8h2.6l.35 1.7 1.3.75 1.65-.6 1.3 2.25-1.3 1.15v1.5l1.3 1.15-1.3 2.25-1.65-.6-1.3.75-.35 1.7H6.7l-.35-1.7-1.3-.75-1.65.6-1.3-2.25 1.3-1.15v-1.5L1.4 5.9l1.3-2.25 1.65.6 1.3-.75z"/>' +
  '<circle cx="8" cy="8" r="2.1"/>'
);

export const iconTerminal = svg(
  '<path d="M3 4.5 6.5 8 3 11.5"/><path d="M8.5 11.5H13"/>'
);

// Spark / four-point diamond — the AI mark
export const iconAI = svg(
  '<path d="M8 1.5 9.6 6.4 14.5 8 9.6 9.6 8 14.5 6.4 9.6 1.5 8 6.4 6.4 8 1.5z"/>'
);

// Two connected nodes — agent
export const iconAgent = svg(
  '<circle cx="4" cy="11" r="2.2"/><circle cx="12" cy="5" r="2.2"/><path d="M5.7 9.4 10.3 6.6"/>'
);

export const iconUser = svg(
  '<circle cx="8" cy="5.5" r="2.8"/><path d="M2.8 13.5c.9-2.4 2.9-3.7 5.2-3.7s4.3 1.3 5.2 3.7"/>'
);

export const iconLock = svg(
  '<rect x="3.5" y="7" width="9" height="6.5" rx="1"/><path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2"/>'
);

// Clean upward arrow — reads better than a plane as a flat grey send glyph.
export const iconSend = svg('<path d="M8 13.5V3.5"/><path d="M3.5 8 8 3.5 12.5 8"/>', '0 0 16 16');

export const iconCopy = svg(
  '<rect x="5.5" y="5.5" width="8" height="8" rx="1"/><path d="M10.5 5.5v-2a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2"/>'
);

export const iconCheck = svg('<path d="M3 8.5 6.5 12 13 4.5"/>');

export const iconWarning = svg(
  '<path d="M8 2 14.5 13.5h-13L8 2z"/><path d="M8 6.5v3.5"/><path d="M8 12.2v.01"/>'
);

export const iconError = svg(
  '<circle cx="8" cy="8" r="6"/><path d="M5.8 5.8l4.4 4.4M10.2 5.8l-4.4 4.4"/>'
);

export const iconBranch = svg(
  '<circle cx="4.5" cy="3.5" r="1.8"/><circle cx="4.5" cy="12.5" r="1.8"/><circle cx="11.5" cy="5.5" r="1.8"/><path d="M4.5 5.3v5.4"/><path d="M11.5 7.3c0 2.2-2.5 3-5 3.2"/>'
);

export const iconUpgrade = svg(
  '<circle cx="8" cy="8" r="6"/><path d="M8 11V5M5.5 7.5 8 5l2.5 2.5"/>'
);

export const iconKey = svg(
  '<circle cx="5" cy="8" r="3"/><path d="M8 8h6M12 8v2.5M14 8v1.5"/>'
);

// Clock with a counter-clockwise arrow — chat history.
export const iconHistory = svg(
  '<path d="M2.5 8a5.5 5.5 0 1 0 1.7-4"/><path d="M2 2v3h3"/><path d="M8 5v3l2 1.5"/>'
);

// Two arrows pointing inward — compact / collapse the conversation.
export const iconCompact = svg(
  '<path d="M7 2v3.5H3.5M9 2v3.5h3.5M7 14v-3.5H3.5M9 14v-3.5h3.5"/>'
);

// Trash — delete a saved chat.
export const iconTrash = svg(
  '<path d="M3 4.5h10M6 4.5V3a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5M5 4.5l.5 8a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1l.5-8"/>'
);

// Picture / image — framed mountain + sun, the media-gen toggle.
export const iconImage = svg(
  '<rect x="2.5" y="3.5" width="11" height="9" rx="1.5"/><circle cx="6" cy="6.5" r="1"/><path d="M3 11.5l3-3 2.5 2.5L11 8l2.5 2.5"/>'
);

// Text (chat) models: a speech bubble with lines.
export const iconText = svg(
  '<path d="M3 3.5h10a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H8.5L5.5 14v-2.5H3a1 1 0 0 1-1-1v-6a1 1 0 0 1 1-1z"/><path d="M5 6.5h6M5 8.5h4"/>'
);

// Video models: a clapper-style frame with a play triangle.
export const iconVideo = svg(
  '<rect x="2" y="3.5" width="12" height="9" rx="1.5"/><path d="M6.8 6.2v3.6L10 8z"/>'
);

/** Wrap an icon string in a span with a CSS class for sizing/coloring. */
export function icon(markup: string, cls = ''): string {
  return `<span class="icon ${cls}">${markup}</span>`;
}
