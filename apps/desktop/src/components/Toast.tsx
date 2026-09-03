import { useEffect, useRef } from "react";

export interface ToastMessage {
  id: number;
  text: string;
  type?: "info" | "success" | "warning";
}

interface ToastProps {
  toast: ToastMessage | null;
  onDismiss: () => void;
}

export default function Toast({ toast, onDismiss }: ToastProps) {
  const onDismissRef = useRef(onDismiss);
  onDismissRef.current = onDismiss;

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => {
      onDismissRef.current();
    }, 2500);
    return () => clearTimeout(timer);
  }, [toast?.id]);

  if (!toast) return null;

  return (
    <div className="toast-container" role="status" aria-live="polite">
      <div
        className={`toast-pill toast-${toast.type ?? "info"}`}
        onClick={() => onDismissRef.current()}
        title="Click to dismiss"
        role="button"
        tabIndex={0}
      >
        <span className="toast-text">{toast.text}</span>
      </div>
    </div>
  );
}
