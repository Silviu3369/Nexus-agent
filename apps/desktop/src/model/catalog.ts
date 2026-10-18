// What this build of Nexus can and cannot do, for the Tools, Agents and
// Models views. These are facts about the Host's code, not activity: they
// mirror the tool definitions in `crates/nexus-host/src/runtime/chat.rs` and
// `crates/nexus-providers/src/web.rs`. When the Host publishes its own
// catalog, this module is the adapter to replace.

export type ToolId =
  | "web"
  | "files"
  | "documents"
  | "records"
  | "memory"
  | "browser"
  | "terminal"
  | "computer"
  | "git"
  | "apis"
  | "mcp";

export interface ToolInfo {
  id: ToolId;
  name: string;
  /** The tool name the model sees, when it has one. */
  hostTool: string | null;
  available: boolean;
  permission: string;
  limits: string[];
  /** Why it is not available, for tools that are not. */
  reason?: string;
  planned?: string;
}

export const TOOLS: ToolInfo[] = [
  {
    id: "web",
    name: "Web read",
    hostTool: "fetch_url",
    available: true,
    permission: "Automatic · read-only",
    limits: [
      "GET only, http and https",
      "Public addresses only: private, loopback, link-local and metadata addresses are refused, including after a redirect",
      "Up to 5 redirects, each checked again",
      "2 MiB per page, 20 s, text content types only",
      "At most 20,000 characters of text go back to the model",
      "Up to 4 reads per reply",
      "Page text is data for the model, never instructions, and is not stored",
    ],
  },
  {
    id: "files",
    name: "Files · Notes",
    hostTool: "propose_note",
    available: true,
    permission: "Approval for every write",
    limits: [
      "Creates new Markdown notes only, inside the Nexus notes folder",
      "Never overwrites or deletes a file",
      "The exact content, size and destination are shown before you approve",
      "Up to 128 KiB per note; an approval expires after 10 minutes",
      "The written file is verified against its SHA-256",
    ],
  },
  {
    id: "documents",
    name: "Documents · shared folders",
    hostTool: "read_document",
    available: true,
    permission: "Reads automatically in folders you share · your approval for every new file",
    limits: [
      "Only folders you share here; anything outside them, and any link, is never followed",
      "Reads text, Word (.docx), PDF, Excel and CSV: up to 20 MB per file and 40,000 characters to the model",
      "Each file is read by a separate, limited process (memory, time, no child processes)",
      "Up to 4 listings and 6 reads per reply",
      "New Word, Markdown or text files only in folders you allow writing to, each one approved by you; nothing is ever overwritten",
      "What a document says is data for the model, never instructions",
      "The text it reads goes to your AI provider, like the rest of the task",
    ],
  },
  {
    id: "records",
    name: "Records · Jobs",
    hostTool: "find_records",
    available: true,
    permission: "Writes directly · every change shown and undoable",
    limits: [
      "Your job applications: looked up, added and updated when you tell Nexus in a task",
      "Searches before adding; a duplicate is refused",
      "Every value is checked: dates, statuses, links",
      "At most 6 lookups and 10 changes per reply",
      "Never deletes a record; deleting is yours",
      "Changes only because you asked or told it a fact, never because a page or document says so",
    ],
  },
  {
    id: "memory",
    name: "Memory",
    hostTool: "propose_memory",
    available: true,
    permission: "Your approval for every fact",
    limits: [
      "Proposes facts only; nothing is remembered until you approve it",
      "Up to 3 proposals per reply, each 1 to 1,000 characters",
      "Personal facts go with every request; project facts only with that project's requests",
      "At most 40 approved facts, about 6,000 characters, per request, newest first",
      "A fact proposed after reading a web page names that page, so you can tell where it came from",
      "Facts are data for the model, never instructions; your current message wins",
    ],
  },
  {
    id: "browser",
    name: "Browser control",
    hostTool: null,
    available: false,
    permission: "—",
    limits: [],
    reason: "Nexus cannot open or drive a browser. It can only read public pages (Web read).",
    planned: "Planned with process isolation (M3).",
  },
  {
    id: "terminal",
    name: "Terminal",
    hostTool: null,
    available: false,
    permission: "—",
    limits: [],
    reason: "No command execution. Arbitrary code stays disabled until the isolation tests pass.",
    planned: "Planned with process isolation (M3).",
  },
  {
    id: "computer",
    name: "Computer control",
    hostTool: null,
    available: false,
    permission: "—",
    limits: [],
    reason: "Nexus does not move the mouse, type, or change Windows settings.",
    planned: "Planned after isolation is verified (M3).",
  },
  {
    id: "git",
    name: "Git",
    hostTool: null,
    available: false,
    permission: "—",
    limits: [],
    reason: "No repository access yet.",
    planned: "Planned with the coding agent (M3).",
  },
  {
    id: "apis",
    name: "APIs",
    hostTool: null,
    available: false,
    permission: "—",
    limits: [],
    reason: "No API tools beyond the AI provider and Web read.",
  },
  {
    id: "mcp",
    name: "MCP servers",
    hostTool: "mcp<n>_<tool>",
    available: true,
    permission: "Tools you enable · read-only automatic, others ask you each call",
    limits: [
      "Remote servers only, over https at public addresses. Local servers run third-party code on this PC and stay off until an isolation decision",
      "Nothing is offered to the model until you enable a tool, one by one",
      "A tool the server declares read-only runs when the model calls it; that is the server's claim, which Nexus cannot verify, so enable only servers you trust",
      "Any other tool can change things: each call waits for your approval of the exact call (server, tool as enabled, arguments), is made once, and is never sent again if its outcome is unclear; then the task continues",
      "The definition you enabled is pinned: if the server changes a tool's description, parameters or hints, the tool is off until you enable it again",
      "Up to 4 calls per reply, 30 s each; at most 20,000 characters of a result go back to the model",
      "A server's key stays in the Windows vault and is sent only to that server, in its Authorization header; changing the address drops it",
      "Descriptions and results are the server's data for the model, never instructions or permissions",
    ],
  },
];

export interface ProviderInfo {
  id: string;
  name: string;
  /** `ConnectionView.provider` for this provider; `null` if none exists. */
  stored: string | null;
  /** Whether this build has an adapter for it. */
  adapter: boolean;
  note: string;
}

export const PROVIDERS: ProviderInfo[] = [
  { id: "gemini", name: "Google Gemini", stored: "Gemini", adapter: true, note: "Streaming, tools, thinking level (Gemini 3.x); keys start with AIza" },
  { id: "openai", name: "OpenAI", stored: "OpenAI", adapter: true, note: "Responses API, not stored by OpenAI (store: false), reasoning effort" },
  { id: "anthropic", name: "Anthropic (Claude)", stored: "Anthropic", adapter: true, note: "Messages API, tools, effort; keys start with sk-ant-" },
  { id: "deepseek", name: "DeepSeek", stored: "DeepSeek", adapter: true, note: "Through its Anthropic-compatible endpoint; models from its own list" },
  { id: "local", name: "Local models", stored: null, adapter: false, note: "No local runtime in this build" },
];

export interface PlannedAgent {
  name: string;
  role: string;
  planned: string;
}

/** Specialised agents the roadmap names. None of them runs today. */
export const PLANNED_AGENTS: PlannedAgent[] = [
  { name: "Research Agent", role: "Multi-source reading with citations", planned: "After memory and documents (M5)" },
  { name: "System Agent", role: "Controlled operations on this PC", planned: "M3, after isolation" },
  { name: "Verification Agent", role: "Independent checks of results", planned: "With the orchestrator" },
];

/** The one agent that exists: the assistant, defined by the persona. */
export const ASSISTANT = {
  id: "assistant",
  role: "General assistant",
  tools: ["web", "files", "documents", "records", "memory", "mcp"] as ToolId[],
  permissions: [
    "Read public web pages (GET), automatically",
    "Propose new notes; each write needs your approval",
    "Propose facts to remember; each one needs your approval",
    "Call the MCP tools you enabled: read-only ones automatically, others only after you approve each call",
    "Read documents in the folders you share; each new file it proposes needs your approval",
    "Keep your job applications: adds and updates them directly, every change undoable",
    "No commands, no PC control, nothing outside the folders you share",
    "Persona changes tone and style only, never rights",
  ],
};
