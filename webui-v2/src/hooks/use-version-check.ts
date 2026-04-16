import { useEffect, useRef } from "react";
import { getUUID } from "../lib/device";
import { checkVersion } from "../lib/cell-api";

const ONE_HOUR_MS = 60 * 60 * 1000;

export function useVersionCheck() {
  const checked = useRef(false);

  useEffect(() => {
    if (checked.current) return;
    checked.current = true;

    const uuid = getUUID();

    // Initial check
    checkVersion(uuid);

    // Hourly repeat
    const handle = setInterval(() => checkVersion(uuid), ONE_HOUR_MS);
    return () => clearInterval(handle);
  }, []);
}
