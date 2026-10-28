/// Swipe detection for a container. Fires on release only, and only on the
/// dominant axis, so a vertical scroll never turns into a horizontal swipe (or
/// the other way round). Handlers are optional; a direction with no handler is
/// simply ignored.
export type SwipeHandlers = {
  onleft?: () => void;
  onright?: () => void;
  onup?: () => void;
  ondown?: () => void;
  /// Distance in pixels before a release counts. Defaults to 60.
  threshold?: number;
};

export function swipe(node: HTMLElement, handlers: SwipeHandlers) {
  let startX = 0;
  let startY = 0;
  let tracking = false;

  function down(event: PointerEvent) {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    tracking = true;
    startX = event.clientX;
    startY = event.clientY;
  }

  function up(event: PointerEvent) {
    if (!tracking) return;
    tracking = false;
    const dx = event.clientX - startX;
    const dy = event.clientY - startY;
    const threshold = handlers.threshold ?? 60;
    if (Math.abs(dx) > Math.abs(dy)) {
      if (dx <= -threshold) handlers.onleft?.();
      else if (dx >= threshold) handlers.onright?.();
    } else if (dy <= -threshold) {
      handlers.onup?.();
    } else if (dy >= threshold) {
      handlers.ondown?.();
    }
  }

  function cancel() {
    tracking = false;
  }

  node.addEventListener("pointerdown", down);
  node.addEventListener("pointerup", up);
  node.addEventListener("pointercancel", cancel);

  return {
    destroy() {
      node.removeEventListener("pointerdown", down);
      node.removeEventListener("pointerup", up);
      node.removeEventListener("pointercancel", cancel);
    },
  };
}
