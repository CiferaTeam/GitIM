import { useCallback, useEffect, useReducer, useRef } from "react";
import { applyFrame, cloneInitial, stateAtFrame } from "./scenario";
import type { DemoScenario, DemoState } from "./types";

export type DemoPlayerStatus = "idle" | "playing" | "paused" | "finished";

interface PlayerState {
  status: DemoPlayerStatus;
  frameIndex: number;
  state: DemoState;
  reducedMotion: boolean;
}

type PlayerAction =
  | { type: "PLAY" }
  | { type: "PAUSE" }
  | { type: "RESET" }
  | { type: "NEXT" }
  | { type: "PREV" }
  | { type: "GO_TO"; index: number }
  | { type: "TICK" }
  | { type: "FINISH" }
  | { type: "SET_REDUCED_MOTION"; value: boolean };

function playerReducer(
  state: PlayerState,
  action: PlayerAction,
  scenario: DemoScenario,
): PlayerState {
  switch (action.type) {
    case "PLAY": {
      if (state.reducedMotion) return state;
      if (state.status === "finished") {
        return {
          status: "playing",
          frameIndex: -1,
          state: cloneInitial(scenario.initialState),
          reducedMotion: state.reducedMotion,
        };
      }
      return { ...state, status: "playing" };
    }
    case "PAUSE": {
      return { ...state, status: "paused" };
    }
    case "RESET": {
      return {
        status: "idle",
        frameIndex: -1,
        state: cloneInitial(scenario.initialState),
        reducedMotion: state.reducedMotion,
      };
    }
    case "NEXT": {
      const nextIndex = state.frameIndex + 1;
      if (nextIndex >= scenario.frames.length) {
        return {
          ...state,
          status: "finished",
          frameIndex: scenario.frames.length - 1,
        };
      }
      const frame = scenario.frames[nextIndex];
      return {
        ...state,
        frameIndex: nextIndex,
        state: applyFrame(state.state, frame),
        status:
          nextIndex >= scenario.frames.length - 1 && state.status !== "playing"
            ? "paused"
            : state.status === "finished"
              ? "paused"
              : state.status,
      };
    }
    case "PREV": {
      const nextIndex = Math.max(-1, state.frameIndex - 1);
      const nextState =
        nextIndex < 0
          ? cloneInitial(scenario.initialState)
          : stateAtFrame(scenario, nextIndex);
      return {
        ...state,
        frameIndex: nextIndex,
        state: nextState,
        status: "paused",
      };
    }
    case "GO_TO": {
      const nextIndex = Math.min(
        Math.max(0, action.index),
        scenario.frames.length - 1,
      );
      return {
        ...state,
        frameIndex: nextIndex,
        state: stateAtFrame(scenario, nextIndex),
        status: state.status === "playing" ? "playing" : "paused",
      };
    }
    case "TICK": {
      const nextIndex = state.frameIndex + 1;
      if (nextIndex >= scenario.frames.length) {
        return { ...state, status: "finished" };
      }
      const frame = scenario.frames[nextIndex];
      return {
        ...state,
        frameIndex: nextIndex,
        state: applyFrame(state.state, frame),
      };
    }
    case "FINISH": {
      return { ...state, status: "finished" };
    }
    case "SET_REDUCED_MOTION": {
      return {
        ...state,
        reducedMotion: action.value,
        status: action.value ? "paused" : state.status,
      };
    }
    default:
      return state;
  }
}

export interface UseDemoPlayerOptions {
  autoplay?: boolean;
  /**
   * Force step-through mode regardless of the prefers-reduced-motion media
   * query. Used by the ?debug=1 escape hatch so every frame can be advanced
   * manually during development and e2e screenshot passes.
   */
  stepMode?: boolean;
  /**
   * frameId → minimum dwell time in ms, typically the narration audio
   * duration. The timer that advances AWAY from frame N waits at least
   * frameDurations[frameN], so a narrated frame is never cut off mid-clip;
   * frames without an entry keep the pacing the scenario delayMs gives them.
   */
  frameDurations?: Record<string, number>;
}

export interface UseDemoPlayerReturn {
  status: DemoPlayerStatus;
  frameIndex: number;
  state: DemoState;
  reducedMotion: boolean;
  currentFrame: import("./types").DemoFrame | null;
  isFirstFrame: boolean;
  isLastFrame: boolean;
  play: () => void;
  pause: () => void;
  reset: () => void;
  next: () => void;
  prev: () => void;
  goTo: (index: number) => void;
}

export function useDemoPlayer(
  scenario: DemoScenario,
  options: UseDemoPlayerOptions = {},
): UseDemoPlayerReturn {
  const { autoplay = false, stepMode = false, frameDurations = {} } = options;

  const prefersReducedMotion =
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const stepForced = stepMode || prefersReducedMotion;

  const [state, dispatch] = useReducer(
    (s: PlayerState, a: PlayerAction) => playerReducer(s, a, scenario),
    {
      status: stepForced ? "paused" : "idle",
      frameIndex: -1,
      state: cloneInitial(scenario.initialState),
      reducedMotion: stepForced,
    },
  );

  useEffect(() => {
    if (stepMode) return; // debug step mode always wins over the media query
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const handler = (e: MediaQueryListEvent) => {
      dispatch({ type: "SET_REDUCED_MOTION", value: e.matches });
    };
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [stepMode]);

  const didAutoplay = useRef(false);
  useEffect(() => {
    if (!autoplay || didAutoplay.current) return;
    if (stepForced) return;
    didAutoplay.current = true;
    dispatch({ type: "PLAY" });
  }, [autoplay, stepForced]);

  useEffect(() => {
    if (state.status !== "playing" || state.reducedMotion) return;
    if (state.frameIndex >= scenario.frames.length - 1) return;

    const currentFrame =
      state.frameIndex >= 0 ? scenario.frames[state.frameIndex] : null;
    const nextFrame = scenario.frames[state.frameIndex + 1];
    // The dwell before advancing belongs to the CURRENT frame: it must be at
    // least as long as the current frame's narration clip, otherwise long
    // clips get cut off when the next frame has a short delayMs.
    const dwell = Math.max(
      nextFrame.delayMs,
      currentFrame ? (frameDurations[currentFrame.id] ?? 0) : 0,
    );
    const handle = window.setTimeout(() => {
      dispatch({ type: "TICK" });
    }, dwell);
    return () => window.clearTimeout(handle);
  }, [state.status, state.frameIndex, state.reducedMotion, scenario.frames, frameDurations]);

  useEffect(() => {
    if (state.status !== "playing" || state.reducedMotion) return;
    if (state.frameIndex < scenario.frames.length - 1) return;
    if (state.frameIndex < 0) return;

    const last = scenario.frames[scenario.frames.length - 1];
    const dwell = Math.max(last.delayMs, frameDurations[last.id] ?? 0);
    const handle = window.setTimeout(() => {
      dispatch({ type: "FINISH" });
    }, dwell);
    return () => window.clearTimeout(handle);
  }, [state.status, state.frameIndex, state.reducedMotion, scenario.frames, frameDurations]);

  const play = useCallback(() => dispatch({ type: "PLAY" }), []);
  const pause = useCallback(() => dispatch({ type: "PAUSE" }), []);
  const reset = useCallback(() => dispatch({ type: "RESET" }), []);
  const next = useCallback(() => dispatch({ type: "NEXT" }), []);
  const prev = useCallback(() => dispatch({ type: "PREV" }), []);
  const goTo = useCallback(
    (index: number) => dispatch({ type: "GO_TO", index }),
    [],
  );

  const currentFrame =
    state.frameIndex >= 0 && state.frameIndex < scenario.frames.length
      ? scenario.frames[state.frameIndex]
      : null;

  return {
    status: state.status,
    frameIndex: state.frameIndex,
    state: state.state,
    reducedMotion: state.reducedMotion,
    currentFrame,
    isFirstFrame: state.frameIndex <= -1,
    isLastFrame: state.frameIndex >= scenario.frames.length - 1,
    play,
    pause,
    reset,
    next,
    prev,
    goTo,
  };
}
