/* tslint:disable */
/* eslint-disable */

export function buildRebaseCommitMsg(mappings_json: string, additions_json: string): string;

export function dmFilename(a: string, b: string): string;

export function extractLinks(body: string): any;

export function extractMentions(body: string): any;

export function formatEvent(line_number: bigint, author: string, timestamp: string, event_type: string, meta_json: string): string;

export function formatMessage(line_number: bigint, point_to: bigint, author: string, timestamp: string, body: string): string;

export function githubIdentityFromUserJson(user_json: string): any;

export function mergeChannelMeta(local_yaml: string, remote_yaml: string): any;

export function parseCardMeta(yaml: string): any;

export function parseThread(text: string): any;

export function renumberBatch(batch: string, max_existing: bigint): string;

export function resolveContentPure(additions_json: string, remote_json: string): any;

export function stringifyCardMeta(meta: any): string;

export function validateAppend(existing: string, new_lines: string, users: any, senders: any): void;

export function validateCardId(card_id: string): void;

export function validateCardLabels(labels: any): void;

export function validateCardMeta(meta: any): void;

export function validateChannelMeta(yaml: string): any;

export function validateJoin(author: string, targets: any, users: any, members: any): void;

export function validateLeave(author: string, targets: any, users: any, members: any): void;

export function validateUserMeta(yaml: string): any;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly githubIdentityFromUserJson: (a: number, b: number) => [number, number, number];
    readonly parseThread: (a: number, b: number) => [number, number, number];
    readonly formatMessage: (a: bigint, b: bigint, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly formatEvent: (a: bigint, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number, number];
    readonly validateAppend: (a: number, b: number, c: number, d: number, e: any, f: any) => [number, number];
    readonly validateJoin: (a: number, b: number, c: any, d: any, e: any) => [number, number];
    readonly validateLeave: (a: number, b: number, c: any, d: any, e: any) => [number, number];
    readonly validateUserMeta: (a: number, b: number) => [number, number, number];
    readonly validateChannelMeta: (a: number, b: number) => [number, number, number];
    readonly parseCardMeta: (a: number, b: number) => [number, number, number];
    readonly stringifyCardMeta: (a: any) => [number, number, number, number];
    readonly validateCardMeta: (a: any) => [number, number];
    readonly validateCardId: (a: number, b: number) => [number, number];
    readonly validateCardLabels: (a: any) => [number, number];
    readonly extractMentions: (a: number, b: number) => [number, number, number];
    readonly extractLinks: (a: number, b: number) => [number, number, number];
    readonly dmFilename: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly renumberBatch: (a: number, b: number, c: bigint) => [number, number, number, number];
    readonly mergeChannelMeta: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly buildRebaseCommitMsg: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly resolveContentPure: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
