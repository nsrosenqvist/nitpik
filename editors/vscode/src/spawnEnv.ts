import type { AuthService } from "./auth";
import type { ShimServer } from "./shim/server";
import { getConfig } from "./config";

/**
 * Build the environment for a spawned `nitpik serve …` process.
 *
 * Always passes the license key. In `copilot` mode it points nitpik's
 * OpenAI-compatible backend at the localhost shim (so reviews use the editor's
 * model); in `byom` mode it adds nothing, letting nitpik use the user's
 * `.nitpik.toml` provider.
 */
export async function buildReviewEnv(auth: AuthService, shim: ShimServer): Promise<Record<string, string>> {
  const cfg = getConfig();
  const env: Record<string, string> = {};

  const key = await auth.getKey();
  if (key) {
    env.NITPIK_LICENSE_KEY = key;
  }

  if (cfg.modelSource === "copilot") {
    env.NITPIK_PROVIDER = "openai-compatible";
    env.NITPIK_BASE_URL = shim.baseUrl;
    env.NITPIK_API_KEY = shim.secret;
    // The shim selects the Copilot model itself; nitpik still needs a model
    // name for its openai-compatible backend.
    env.NITPIK_MODEL = cfg.copilotModel || "copilot";
  }

  return env;
}
