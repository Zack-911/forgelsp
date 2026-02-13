/* tslint:disable */
/* eslint-disable */
/**
 * Gets hover information for a function name.
 *
 * Returns JSON with function metadata or null if not found.
 */
export function get_hover(function_name: string): any;
/**
 * Gets all function names as a JSON array of strings.
 */
export function get_all_function_names(): any;
/**
 * Parses a ForgeScript document and returns a JSON result.
 *
 * Returns JSON with `{ functions, diagnostics }`.
 */
export function process_document(text: string): any;
/**
 * Converts a byte offset to (line, character) in the given text.
 */
export function offset_to_position(text: string, offset: number): any;
/**
 * Initializes the WASM module with optional configuration JSON.
 *
 * Call this before any other API. Accepts a JSON string matching the
 * `forgeconfig.json` schema (or an empty string for defaults).
 */
export function init(config_json: string): void;
/**
 * Loads metadata from the configured URLs.
 *
 * Returns a Promise that resolves when metadata is loaded.
 */
export function load_metadata(): Promise<any>;
/**
 * Converts (line, character) to a byte offset in the given text.
 */
export function position_to_offset(text: string, line: number, character: number): any;
/**
 * Extracts semantic tokens for the given ForgeScript text.
 *
 * Returns a JSON string containing relative LSP semantic tokens.
 */
export function get_semantic_tokens(text: string, use_colors: boolean): any;
/**
 * Extracts autocomplete results for the given ForgeScript text and position.
 */
export function get_completions(text: string, line: number, character: number): any;
/**
 * Adds custom function definitions from a JSON string.
 *
 * Accepts a JSON array of custom function objects matching the `CustomFunction` schema.
 */
export function add_custom_functions(json: string): any;
/**
 * Extracts VS Code-specific highlight ranges for the given text.
 *
 * Returns a JSON string containing `Vec<(start, end, color)>`.
 */
export function get_highlight_ranges(text: string): any;
/**
 * Returns the total number of functions loaded.
 */
export function get_function_count(): number;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly add_custom_functions: (a: number, b: number, c: number) => void;
  readonly get_all_function_names: (a: number) => void;
  readonly get_completions: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly get_function_count: (a: number) => void;
  readonly get_highlight_ranges: (a: number, b: number, c: number) => void;
  readonly get_hover: (a: number, b: number, c: number) => void;
  readonly get_semantic_tokens: (a: number, b: number, c: number, d: number) => void;
  readonly init: (a: number, b: number, c: number) => void;
  readonly offset_to_position: (a: number, b: number, c: number) => number;
  readonly position_to_offset: (a: number, b: number, c: number, d: number) => number;
  readonly process_document: (a: number, b: number, c: number) => void;
  readonly load_metadata: () => number;
  readonly __wbindgen_export_0: (a: number) => void;
  readonly __wbindgen_export_1: (a: number, b: number) => number;
  readonly __wbindgen_export_2: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_export_3: WebAssembly.Table;
  readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
  readonly __wbindgen_export_4: (a: number, b: number, c: number) => void;
  readonly __wbindgen_export_5: (a: number, b: number) => void;
  readonly __wbindgen_export_6: (a: number, b: number, c: number, d: number) => void;
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
