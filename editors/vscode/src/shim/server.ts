import * as vscode from "vscode";
import * as http from "node:http";
import { randomBytes, timingSafeEqual } from "node:crypto";
import { log, logError } from "../log";
import {
  type OpenAIChatRequest,
  type ModelEvent,
  runModel,
  selectModel,
  translateMessages,
  translateTools,
} from "./bridge";

/**
 * A localhost OpenAI-compatible endpoint backed by `vscode.lm`.
 *
 * nitpik (spawned with `NITPIK_PROVIDER=openai-compatible` + this base URL +
 * the shared secret) talks to `/v1/chat/completions`; the request is
 * translated to a Copilot model request via `vscode.lm` and streamed back as
 * OpenAI SSE (or a single JSON body). Bound to 127.0.0.1 on a random port;
 * every request must carry `Authorization: Bearer <secret>`.
 */
export class ShimServer {
  private server: http.Server | undefined;
  private _port = 0;
  readonly secret = randomBytes(32).toString("hex");

  constructor(private readonly modelFamily: () => string) {}

  get port(): number {
    return this._port;
  }

  get baseUrl(): string {
    return `http://127.0.0.1:${this._port}/v1`;
  }

  async start(): Promise<void> {
    if (this.server) {
      return;
    }
    this.server = http.createServer((req, res) => void this.handle(req, res));
    await new Promise<void>((resolve, reject) => {
      this.server!.on("error", reject);
      this.server!.listen(0, "127.0.0.1", () => {
        const addr = this.server!.address();
        this._port = typeof addr === "object" && addr ? addr.port : 0;
        log(`shim: listening on ${this.baseUrl}`);
        resolve();
      });
    });
  }

  private authorized(req: http.IncomingMessage): boolean {
    const header = req.headers["authorization"];
    const expected = `Bearer ${this.secret}`;
    if (typeof header !== "string" || header.length !== expected.length) {
      return false;
    }
    return timingSafeEqual(Buffer.from(header), Buffer.from(expected));
  }

  private async handle(req: http.IncomingMessage, res: http.ServerResponse): Promise<void> {
    if (req.method !== "POST" || !req.url?.startsWith("/v1/chat/completions")) {
      res.writeHead(404).end();
      return;
    }
    if (!this.authorized(req)) {
      res.writeHead(401, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ error: { message: "unauthorized", type: "invalid_request_error" } }));
      return;
    }

    let body: OpenAIChatRequest;
    try {
      body = JSON.parse(await readBody(req)) as OpenAIChatRequest;
    } catch {
      res.writeHead(400, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ error: { message: "invalid JSON body", type: "invalid_request_error" } }));
      return;
    }

    const cts = new vscode.CancellationTokenSource();
    req.on("close", () => cts.cancel());

    try {
      const model = await selectModel(this.modelFamily());
      const messages = translateMessages(body.messages);
      const tools = translateTools(body.tools);
      const events = runModel(model, messages, tools, cts.token);
      if (body.stream) {
        await this.streamResponse(res, body.model ?? model.id, events);
      } else {
        await this.jsonResponse(res, body.model ?? model.id, events);
      }
    } catch (err) {
      logError("shim: request failed", err);
      const message = err instanceof Error ? err.message : String(err);
      if (!res.headersSent) {
        res.writeHead(502, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: { message, type: "api_error" } }));
      } else {
        res.end();
      }
    } finally {
      cts.dispose();
    }
  }

  private async jsonResponse(
    res: http.ServerResponse,
    model: string,
    events: AsyncGenerator<ModelEvent>,
  ): Promise<void> {
    let text = "";
    const toolCalls: Array<{ id: string; type: "function"; function: { name: string; arguments: string } }> = [];
    for await (const ev of events) {
      if (ev.kind === "text") {
        text += ev.text;
      } else {
        toolCalls.push({ id: ev.id, type: "function", function: { name: ev.name, arguments: ev.args } });
      }
    }
    const payload = {
      id: `chatcmpl-${randomBytes(8).toString("hex")}`,
      object: "chat.completion",
      created: Math.floor(Date.now() / 1000),
      model,
      choices: [
        {
          index: 0,
          message: {
            role: "assistant",
            content: text || null,
            ...(toolCalls.length > 0 ? { tool_calls: toolCalls } : {}),
          },
          finish_reason: toolCalls.length > 0 ? "tool_calls" : "stop",
        },
      ],
    };
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(payload));
  }

  private async streamResponse(
    res: http.ServerResponse,
    model: string,
    events: AsyncGenerator<ModelEvent>,
  ): Promise<void> {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    const id = `chatcmpl-${randomBytes(8).toString("hex")}`;
    const created = Math.floor(Date.now() / 1000);
    const send = (delta: object, finish: string | null) => {
      const chunk = {
        id,
        object: "chat.completion.chunk",
        created,
        model,
        choices: [{ index: 0, delta, finish_reason: finish }],
      };
      res.write(`data: ${JSON.stringify(chunk)}\n\n`);
    };

    send({ role: "assistant" }, null);
    let toolIndex = 0;
    let sawTool = false;
    for await (const ev of events) {
      if (ev.kind === "text") {
        send({ content: ev.text }, null);
      } else {
        sawTool = true;
        send(
          {
            tool_calls: [
              {
                index: toolIndex++,
                id: ev.id,
                type: "function",
                function: { name: ev.name, arguments: ev.args },
              },
            ],
          },
          null,
        );
      }
    }
    send({}, sawTool ? "tool_calls" : "stop");
    res.write("data: [DONE]\n\n");
    res.end();
  }

  dispose(): void {
    this.server?.close();
    this.server = undefined;
  }
}

function readBody(req: http.IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}
