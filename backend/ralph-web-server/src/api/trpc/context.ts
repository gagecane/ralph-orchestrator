/**
 * TRPC Context and Base Configuration
 *
 * Defines the shared Context, createContext factory, and the base tRPC
 * builder (router, publicProcedure) used by every router module.
 */

import { initTRPC } from "@trpc/server";
import { BetterSQLite3Database } from "drizzle-orm/better-sqlite3";
import {
  TaskRepository,
  SettingsRepository,
  TaskLogRepository,
  CollectionRepository,
} from "../../repositories";
import { SettingsService } from "../../services/SettingsService";
import { TaskBridge } from "../../services/TaskBridge";
import { LoopsManager } from "../../services/LoopsManager";
import { PlanningService } from "../../services/PlanningService";
import { CollectionService } from "../../services/CollectionService";
import * as schema from "../../db/schema";

/**
 * Context passed to all TRPC procedures
 */
export interface Context {
  taskRepository: TaskRepository;
  taskLogRepository: TaskLogRepository;
  settingsService: SettingsService;
  collectionService: CollectionService;
  taskBridge?: TaskBridge;
  loopsManager?: LoopsManager;
  planningService?: PlanningService;
}

/**
 * Create context from database instance
 * @param db - Database instance
 * @param taskBridge - Optional TaskBridge for task execution
 * @param loopsManager - Optional LoopsManager for loop operations
 * @param planningService - Optional PlanningService for planning sessions
 */
export function createContext(
  db: BetterSQLite3Database<typeof schema>,
  taskBridge?: TaskBridge,
  loopsManager?: LoopsManager,
  planningService?: PlanningService
): Context {
  const settingsRepository = new SettingsRepository(db);
  const collectionRepository = new CollectionRepository(db);
  return {
    taskRepository: new TaskRepository(db),
    taskLogRepository: new TaskLogRepository(db),
    settingsService: new SettingsService(settingsRepository),
    collectionService: new CollectionService(collectionRepository),
    taskBridge,
    loopsManager,
    planningService,
  };
}

const t = initTRPC.context<Context>().create();

export const router = t.router;
export const publicProcedure = t.procedure;
