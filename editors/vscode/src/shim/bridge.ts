import * as vscode from "vscode";

/** Minimal subset of the OpenAI chat-completion request we translate. */
export interface OpenAIMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string | Array<{ type: string; text?: string }> | null;
  tool_calls?: Array<{ id: string; type: "function"; function: { name: string; arguments: string } }>;
  tool_call_id?: string;
}

export interface OpenAITool {
  type: "function";
  function: { name: string; description?: string; parameters?: unknown };
}

export interface OpenAIChatRequest {
  model?: string;
  messages: OpenAIMessage[];
  tools?: OpenAITool[];
  stream?: boolean;
}

/** A single event streamed back from the language model. */
export type ModelEvent =
  | { kind: "text"; text: string }
  | { kind: "tool"; id: string; name: string; args: string };

function textOf(content: OpenAIMessage["content"]): string {
  if (typeof content === "string") {
    return content;
  }
  if (Array.isArray(content)) {
    return content.map((p) => p.text ?? "").join("");
  }
  return "";
}

/**
 * Translate OpenAI chat messages into `vscode.LanguageModelChatMessage[]`.
 *
 * vscode.lm has only User/Assistant roles: system messages are folded into a
 * User message, tool calls become `LanguageModelToolCallPart`s on an Assistant
 * message, and `role: "tool"` results become `LanguageModelToolResultPart`s on
 * a User message.
 */
export function translateMessages(messages: OpenAIMessage[]): vscode.LanguageModelChatMessage[] {
  const out: vscode.LanguageModelChatMessage[] = [];
  for (const m of messages) {
    switch (m.role) {
      case "system":
      case "user": {
        const text = textOf(m.content);
        if (text) {
          out.push(vscode.LanguageModelChatMessage.User(text));
        }
        break;
      }
      case "assistant": {
        const parts: Array<vscode.LanguageModelTextPart | vscode.LanguageModelToolCallPart> = [];
        const text = textOf(m.content);
        if (text) {
          parts.push(new vscode.LanguageModelTextPart(text));
        }
        for (const tc of m.tool_calls ?? []) {
          let input: object = {};
          try {
            input = tc.function.arguments ? JSON.parse(tc.function.arguments) : {};
          } catch {
            input = {};
          }
          parts.push(new vscode.LanguageModelToolCallPart(tc.id, tc.function.name, input));
        }
        if (parts.length > 0) {
          out.push(vscode.LanguageModelChatMessage.Assistant(parts));
        }
        break;
      }
      case "tool": {
        const result = new vscode.LanguageModelToolResultPart(m.tool_call_id ?? "", [
          new vscode.LanguageModelTextPart(textOf(m.content)),
        ]);
        out.push(vscode.LanguageModelChatMessage.User([result]));
        break;
      }
    }
  }
  return out;
}

export function translateTools(tools: OpenAITool[] | undefined): vscode.LanguageModelChatTool[] {
  return (tools ?? []).map((t) => ({
    name: t.function.name,
    description: t.function.description ?? "",
    inputSchema: t.function.parameters as object | undefined,
  }));
}

/**
 * Pick a Copilot chat model. `family` may be "" to take the first available.
 * Throws when no model is available (e.g. Copilot not installed / no consent).
 */
export async function selectModel(family: string): Promise<vscode.LanguageModelChat> {
  const selector: vscode.LanguageModelChatSelector = family
    ? { vendor: "copilot", family }
    : { vendor: "copilot" };
  let models = await vscode.lm.selectChatModels(selector);
  if (models.length === 0 && family) {
    // Fall back to any Copilot model if the requested family is unavailable.
    models = await vscode.lm.selectChatModels({ vendor: "copilot" });
  }
  if (models.length === 0) {
    throw new Error("no Copilot language model is available (is GitHub Copilot installed and authorized?)");
  }
  return models[0];
}

/**
 * Run the model and yield translated events. Tool calls are surfaced as
 * complete `tool` events (vscode.lm delivers whole tool-call parts).
 */
export async function* runModel(
  model: vscode.LanguageModelChat,
  messages: vscode.LanguageModelChatMessage[],
  tools: vscode.LanguageModelChatTool[],
  token: vscode.CancellationToken,
): AsyncGenerator<ModelEvent> {
  const options: vscode.LanguageModelChatRequestOptions = {
    justification: "nitpik runs code review using your Copilot model.",
  };
  if (tools.length > 0) {
    options.tools = tools;
    options.toolMode = vscode.LanguageModelChatToolMode.Auto;
  }

  const response = await model.sendRequest(messages, options, token);
  for await (const part of response.stream) {
    if (part instanceof vscode.LanguageModelTextPart) {
      yield { kind: "text", text: part.value };
    } else if (part instanceof vscode.LanguageModelToolCallPart) {
      yield {
        kind: "tool",
        id: part.callId,
        name: part.name,
        args: JSON.stringify(part.input ?? {}),
      };
    }
  }
}
