/// Long-press to open a menu, with a right-click doing the same thing on
/// desktop. The press is cancelled by a scroll or a moving finger, and the
/// click that follows a long press is swallowed so the row does not also play.
export function longpress(node: HTMLElement, handler: () => void) {
  const DELAY = 450;
  const SLOP = 10;

  let timer: ReturnType<typeof setTimeout> | null = null;
  let startX = 0;
  let startY = 0;
  let fired = false;

  function cancel() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  function down(event: PointerEvent) {
    if (event.button !== 0) return;
    fired = false;
    startX = event.clientX;
    startY = event.clientY;
    cancel();
    timer = setTimeout(() => {
      timer = null;
      fired = true;
      navigator.vibrate?.(12);
      handler();
    }, DELAY);
  }

  function move(event: PointerEvent) {
    if (timer === null) return;
    if (Math.hypot(event.clientX - startX, event.clientY - startY) > SLOP) {
      cancel();
    }
  }

  // Capture, so the button inside the row never sees the click that ends a
  // long press.
  function click(event: MouseEvent) {
    if (!fired) return;
    fired = false;
    event.preventDefault();
    event.stopPropagation();
  }

  function contextmenu(event: MouseEvent) {
    event.preventDefault();
    handler();
  }

  node.addEventListener("pointerdown", down);
  node.addEventListener("pointermove", move);
  node.addEventListener("pointerup", cancel);
  node.addEventListener("pointercancel", cancel);
  node.addEventListener("pointerleave", cancel);
  node.addEventListener("click", click, true);
  node.addEventListener("contextmenu", contextmenu);

  return {
    destroy() {
      cancel();
      node.removeEventListener("pointerdown", down);
      node.removeEventListener("pointermove", move);
      node.removeEventListener("pointerup", cancel);
      node.removeEventListener("pointercancel", cancel);
      node.removeEventListener("pointerleave", cancel);
      node.removeEventListener("click", click, true);
      node.removeEventListener("contextmenu", contextmenu);
    },
  };
}
