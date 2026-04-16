import { useEffect, useRef } from "react";
import { getUUID } from "../lib/device";
import { checkVersion } from "../lib/cell-api";

const ONE_HOUR_MS = 60 * 60 * 1000;

export function useVersionCheck() {
  const checked = useRef(false);

  useEffect(() => {
    const uuid = getUUID();

    if (!checked.current) {
      checked.current = true;
      checkVersion(uuid);
    }

    const handle = setInterval(() => checkVersion(uuid), ONE_HOUR_MS);
    return () => clearInterval(handle);
  }, []);
}
