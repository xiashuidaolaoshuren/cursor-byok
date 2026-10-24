import { buildChatBody } from "./openai_chat.ts";
import type { LlmRequest } from "../provider.ts";

function assert(condition: unknown, message = "assertion failed"): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEquals(actual: unknown, expected: unknown): void {
  const left = JSON.stringify(actual);
  const right = JSON.stringify(expected);
  if (left !== right) throw new Error(`expected ${right}, received ${left}`);
}

function request(messages: LlmRequest["messages"]): LlmRequest {
  return {
    instructions: "",
    messages,
    tools: [],
    reasoning: { enabled: false, effort: null },
    latency: "standard",
    maxOutputTokens: null,
    cacheKey: "conversation-1",
  };
}

function body(messages: LlmRequest["messages"]): Array<Record<string, unknown>> {
  return buildChatBody({
    url: "https://example.com/chat",
    model: "model",
    request: request(messages),
  }).messages as Array<Record<string, unknown>>;
}

Deno.test("tool images travel as user content after the tool batch", () => {
  const messages = body([
    {
      role: "assistant",
      text: "",
      thinking: "",
      replayState: null,
      toolCalls: [{ index: 0, callId: "call-1", name: "look", arguments: {} }],
    },
    {
      role: "tool",
      callId: "call-1",
      name: "look",
      content: "MCP image: image/png (4 bytes)",
      isError: false,
      parts: [
        { type: "text", text: "MCP image: image/png (4 bytes)" },
        { type: "image", mediaType: "image/png", dataBase64: "AQIDBA==" },
      ],
    },
  ]);

  assertEquals(messages.map((message) => message.role), ["assistant", "tool", "user"]);
  assertEquals(messages[1].content, "MCP image: image/png (4 bytes)");
  const content = messages[2].content as Array<Record<string, unknown>>;
  assert(String(content[0].text).includes("call-1"), "image content must name its tool call");
  assertEquals(content[1], {
    type: "image_url",
    image_url: { url: "data:image/png;base64,AQIDBA==" },
  });
});

Deno.test("text-only tool results add no extra user message", () => {
  const messages = body([
    {
      role: "tool",
      callId: "call-1",
      name: "grep",
      content: "result",
      isError: false,
      parts: [],
    },
  ]);

  assertEquals(messages, [{ role: "tool", content: "result", tool_call_id: "call-1" }]);
});
