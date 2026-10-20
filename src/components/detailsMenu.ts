import {
  useCallback,
  useEffect,
  useRef,
  type SyntheticEvent,
} from "react";

const OPEN_MENU_SELECTOR = "details.terminal-split-menu[open]";

export function useExclusiveDetailsMenu() {
  const openDetailsRef = useRef<HTMLDetailsElement | null>(null);
  const cleanupRef = useRef<(() => void) | null>(null);

  const removeDocumentListeners = useCallback(() => {
    cleanupRef.current?.();
    cleanupRef.current = null;
  }, []);

  useEffect(() => removeDocumentListeners, [removeDocumentListeners]);

  return useCallback(
    (event: SyntheticEvent<HTMLDetailsElement>) => {
      const details = event.currentTarget;

      if (!details.open) {
        if (openDetailsRef.current === details) {
          openDetailsRef.current = null;
          removeDocumentListeners();
        }
        return;
      }

      document
        .querySelectorAll<HTMLDetailsElement>(OPEN_MENU_SELECTOR)
        .forEach((otherDetails) => {
          if (otherDetails !== details) {
            otherDetails.removeAttribute("open");
          }
        });

      removeDocumentListeners();
      openDetailsRef.current = details;

      const handlePointerDown = (pointerEvent: PointerEvent) => {
        if (
          !(pointerEvent.target instanceof Node) ||
          !details.contains(pointerEvent.target)
        ) {
          details.removeAttribute("open");
        }
      };
      const handleKeyDown = (keyboardEvent: KeyboardEvent) => {
        if (keyboardEvent.key === "Escape") {
          details.removeAttribute("open");
        }
      };

      document.addEventListener("pointerdown", handlePointerDown);
      document.addEventListener("keydown", handleKeyDown);
      cleanupRef.current = () => {
        document.removeEventListener("pointerdown", handlePointerDown);
        document.removeEventListener("keydown", handleKeyDown);
      };
    },
    [removeDocumentListeners],
  );
}
