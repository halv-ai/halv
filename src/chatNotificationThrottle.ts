/**
 * Limits repeat banners. Each key has its own window, so a finished turn
 * never blocks an error, and one conversation never blocks another.
 */
export function createNotificationThrottle(windowMs: number) {
  const lastAt = new Map<string, number>();
  return {
    /** True, and starts the window, when the key has no open window. */
    allow(key: string, now: number) {
      for (const [other, at] of lastAt) {
        if (now - at >= windowMs) lastAt.delete(other);
      }
      const last = lastAt.get(key);
      if (last !== undefined && now - last < windowMs) {
        return false;
      }
      lastAt.set(key, now);
      return true;
    },
    size() {
      return lastAt.size;
    },
  };
}

export type NotificationThrottle = ReturnType<typeof createNotificationThrottle>;
