/**
 * No-op console helper.
 */
declare module "mechanics:console" {
  export interface MechanicsConsole {
    log(...args: unknown[]): void;
    info(...args: unknown[]): void;
    warn(...args: unknown[]): void;
    error(...args: unknown[]): void;
    debug(...args: unknown[]): void;
  }

  const console: MechanicsConsole;

  export default console;
}
