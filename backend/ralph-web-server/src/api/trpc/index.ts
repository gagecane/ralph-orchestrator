/**
 * TRPC Router Configuration
 *
 * Assembles the main appRouter from per-domain router modules.
 * Each router lives in `./routers/` so this file stays slim.
 */

import { router } from "./context";
import { taskRouter } from "./routers/task";
import { hatRouter } from "./routers/hat";
import { loopsRouter } from "./routers/loops";
import { collectionRouter } from "./routers/collection";
import { configRouter } from "./routers/config";
import { presetsRouter } from "./routers/presets";
import { planningRouter } from "./routers/planning";

// Re-export shared context/builder primitives
export { createContext, router, publicProcedure } from "./context";
export type { Context } from "./context";

// Re-export individual routers for targeted testing
export { taskRouter } from "./routers/task";
export { hatRouter } from "./routers/hat";
export { loopsRouter } from "./routers/loops";
export { collectionRouter } from "./routers/collection";
export { configRouter } from "./routers/config";
export {
  presetsRouter,
  getBuiltinPresets,
  getDirectoryPresets,
  readPresetsFromDir,
} from "./routers/presets";
export type { Preset } from "./routers/presets";
export { planningRouter } from "./routers/planning";

/**
 * Main app router combining all sub-routers
 */
export const appRouter = router({
  task: taskRouter,
  hat: hatRouter,
  loops: loopsRouter,
  collection: collectionRouter,
  presets: presetsRouter,
  config: configRouter,
  planning: planningRouter,
});

export type AppRouter = typeof appRouter;
