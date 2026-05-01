// Inline SVG icons. Stroke-based, monoline.

const Icon = ({ name, className = "ic" }) => {
  const paths = {
    chat: <><path d="M3 12c0-4.4 3.6-8 8-8s8 3.6 8 8-3.6 8-8 8c-1 0-2-.2-2.9-.5L4 21l1.5-3.4C4 16 3 14 3 12Z"/></>,
    canvas: <><rect x="3" y="4" width="18" height="14" rx="2"/><path d="M3 9h18"/><path d="M9 14h6"/></>,
    brain: <><path d="M9 4a3 3 0 0 0-3 3v0a3 3 0 0 0-2 5 3 3 0 0 0 2 5 3 3 0 0 0 3 3 3 3 0 0 0 3-3"/><path d="M15 4a3 3 0 0 1 3 3 3 3 0 0 1 2 5 3 3 0 0 1-2 5 3 3 0 0 1-3 3 3 3 0 0 1-3-3"/><path d="M12 4v16"/></>,
    book: <><path d="M4 4h11a3 3 0 0 1 3 3v13a2 2 0 0 0-2-2H4Z"/><path d="M4 4v14"/></>,
    grid: <><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></>,
    settings: <><circle cx="12" cy="12" r="3"/><path d="M19 12a7 7 0 0 0-.1-1.2l2-1.5-2-3.5-2.4.9a7 7 0 0 0-2-1.2L14 3h-4l-.5 2.5a7 7 0 0 0-2 1.2l-2.4-.9-2 3.5 2 1.5A7 7 0 0 0 5 12c0 .4 0 .8.1 1.2l-2 1.5 2 3.5 2.4-.9a7 7 0 0 0 2 1.2L10 21h4l.5-2.5a7 7 0 0 0 2-1.2l2.4.9 2-3.5-2-1.5c.1-.4.1-.8.1-1.2Z"/></>,
    plus: <><path d="M12 5v14M5 12h14"/></>,
    search: <><circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/></>,
    send: <><path d="m4 12 16-8-6 16-2-7-8-1Z"/></>,
    paperclip: <><path d="m21 11-9 9a5 5 0 0 1-7-7l9-9a3.5 3.5 0 0 1 5 5L10 18a2 2 0 0 1-3-3l8-8"/></>,
    sparkles: <><path d="M12 4v4M12 16v4M4 12h4M16 12h4"/><path d="m6 6 2 2M16 16l2 2M18 6l-2 2M8 16l-2 2"/></>,
    mic: <><rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0"/><path d="M12 18v3"/></>,
    play: <><path d="M6 4v16l14-8Z"/></>,
    pause: <><path d="M7 4v16M17 4v16"/></>,
    expand: <><path d="M4 9V5a1 1 0 0 1 1-1h4M20 9V5a1 1 0 0 0-1-1h-4M4 15v4a1 1 0 0 0 1 1h4M20 15v4a1 1 0 0 1-1 1h-4"/></>,
    close: <><path d="m6 6 12 12M18 6 6 18"/></>,
    chevronR: <><path d="m9 6 6 6-6 6"/></>,
    chevronD: <><path d="m6 9 6 6 6-6"/></>,
    bolt: <><path d="M13 3 5 14h6l-2 7 8-11h-6l2-7Z"/></>,
    layer: <><path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 13 9 5 9-5"/><path d="m3 18 9 5 9-5"/></>,
    pin: <><path d="M12 2a4 4 0 0 0-4 4v5l-2 3h12l-2-3V6a4 4 0 0 0-4-4Z"/><path d="M12 14v8"/></>,
    eye: <><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/></>,
    branch: <><circle cx="6" cy="6" r="2"/><circle cx="6" cy="18" r="2"/><circle cx="18" cy="12" r="2"/><path d="M6 8v8M8 18a6 6 0 0 0 6-6 6 6 0 0 1 4-6"/></>,
  };
  const p = paths[name];
  if (!p) return null;
  return (
    <svg className={className} viewBox="0 0 24 24">{p}</svg>
  );
};

window.Icon = Icon;
