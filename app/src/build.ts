declare const __SANDHILL_BUILD__: { when: string; commit: string };

/** When and from which commit this build was made, like "Oct 12 04:16 UTC" and "a4d87e8". */
export const BUILD = __SANDHILL_BUILD__;

/** "built Oct 12 04:16 UTC (a4d87e8)", to tell one build from another. */
export function buildText(): string {
  return `built ${BUILD.when}${BUILD.commit ? ` (${BUILD.commit})` : ""}`;
}
