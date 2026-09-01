export type ListScrollBounds = {
  scrollHeight: number;
  clientHeight: number;
};

export type ListScrollElement = ListScrollBounds & { scrollTop: number };

export function clampListScrollTop(scrollTop: number, bounds: ListScrollBounds): number {
  return Math.min(Math.max(0, scrollTop), Math.max(0, bounds.scrollHeight - bounds.clientHeight));
}

/**
 * Keeps the workspace list scrolled where the user left it when a view re-renders itself
 * by replacing its markup. The position is only reused for the view it was taken from, so
 * switching tabs still starts at the top.
 */
export function createListScroll(findList: () => ListScrollElement | null, currentView: () => string | null) {
  let saved: { view: string; top: number } | null = null;
  return {
    capture(): void {
      const view = currentView();
      const list = findList();
      if (!view || !list) return;
      saved = { view, top: list.scrollTop };
    },
    restore(view: string): void {
      if (saved?.view !== view) return;
      const list = findList();
      if (!list) return;
      list.scrollTop = clampListScrollTop(saved.top, list);
    }
  };
}
