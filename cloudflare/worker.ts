import { Container } from "@cloudflare/containers";
import { env } from "cloudflare:workers";

export interface Env {
  AFRAMP_CONTAINER: DurableObjectNamespace<AframpContainer>;
  DATABASE_URL: string;
  JWT_SECRET: string;
  WEBHOOK_SECRET: string;
  WALLET_ENCRYPTION_KEY: string;
  STELLAR_SYSTEM_WALLET_ADDRESS: string;
  PAYSTACK_SECRET_KEY: string;
  APP_BIND_ADDR: string;
  STELLAR_HORIZON_URL: string;
  STELLAR_POLL_INTERVAL_SECS: string;
  CORS_ALLOWED_ORIGINS: string;
  COOKIE_SECURE: string;
  COOKIE_SAME_SITE: string;
}

// One named instance prevents duplicate Stellar polling and is sufficient for
// this stateful API. Scale only after moving polling to a singleton job.
export class AframpContainer extends Container {
  defaultPort = 3000;
  sleepAfter = "10m";

  envVars = {
    DATABASE_URL: env.DATABASE_URL,
    JWT_SECRET: env.JWT_SECRET,
    WEBHOOK_SECRET: env.WEBHOOK_SECRET,
    WALLET_ENCRYPTION_KEY: env.WALLET_ENCRYPTION_KEY,
    STELLAR_SYSTEM_WALLET_ADDRESS: env.STELLAR_SYSTEM_WALLET_ADDRESS,
    PAYSTACK_SECRET_KEY: env.PAYSTACK_SECRET_KEY,
    APP_BIND_ADDR: env.APP_BIND_ADDR,
    STELLAR_HORIZON_URL: env.STELLAR_HORIZON_URL,
    STELLAR_POLL_INTERVAL_SECS: env.STELLAR_POLL_INTERVAL_SECS,
    CORS_ALLOWED_ORIGINS: env.CORS_ALLOWED_ORIGINS,
    COOKIE_SECURE: env.COOKIE_SECURE,
    COOKIE_SAME_SITE: env.COOKIE_SAME_SITE,
  };
}

function primaryContainer(env: Env) {
  return env.AFRAMP_CONTAINER.getByName("primary");
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    return primaryContainer(env).fetch(request);
  },

  // Keep the singleton alive so the backend's 60-second Stellar polling loop
  // continues even when the API has no browser traffic.
  async scheduled(_controller: ScheduledController, env: Env): Promise<void> {
    await primaryContainer(env).fetch(
      new Request("http://container.internal/health"),
    );
  },
} satisfies ExportedHandler<Env>;
