/**
 * Minimal ambient declarations for the Node built-ins this test harness uses.
 *
 * The project deliberately does not depend on `@types/node` (it is a
 * browser/Tauri frontend), but the design-token conformance test has to read
 * the CSS sources from disk and resolve its own module path. These
 * declarations cover exactly what the scanner needs — no `any`, no
 * `@ts-ignore`.
 *
 * If `@types/node` is ever added to the project, delete this file: the real
 * declarations supersede it and a duplicate ambient module would fail the
 * type check.
 */
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
  export function existsSync(path: string): boolean;
  export function statSync(path: string): { isDirectory(): boolean; isFile(): boolean };
  export function readdirSync(path: string): string[];
}

declare module "node:url" {
  export function fileURLToPath(url: string | URL): string;
}
