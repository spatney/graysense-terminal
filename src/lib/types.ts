// Mirrors of the Rust DTOs (src-tauri/src/engine/dto.rs). Keep in step with them.

export type BayId = number;
export type Level = 'info' | 'ok' | 'warn' | 'error';
export type ModeId = 'interactive' | 'plan' | 'autopilot';

export interface StatusDto {
  kind: 'booting' | 'idle' | 'thinking' | 'streaming' | 'tool' | 'waiting' | 'compacting' | 'error' | 'offline';
  label: string;
  detail: string | null;
  busy: boolean;
  attention: boolean;
}

export interface CanvasDto {
  instanceId: string;
  canvasId: string;
  title: string;
  state: any;
}

export interface AttachmentDto {
  path: string;
  name: string;
  isDir: boolean;
}

export interface AgentDto {
  sessionId: string | null;
  model: string | null;
  /** Thinking effort chosen for the session (null: the model's default). */
  effort: string | null;
  mode: ModeId;
  status: StatusDto;
  intent: string | null;
  name: string | null;
  deferred: boolean;
  incognito: boolean;
  tokensIn: number;
  tokensOut: number;
  context: [number, number] | null;
  toolCount: number;
  turns: number;
  changedFiles: string[];
  busySince: number | null;
  mission: number | null;
  canvases: CanvasDto[];
  apps: { view: number; title: string }[];
  attachments: AttachmentDto[];
  autoApprove: boolean;
  pending: number;
  memoryUsed: number;
  subagentsActive: number;
  project: string;
  /** Memory scope key of the agent's project (matches MemoryDto.project). */
  projectKey: string;
  sentHistory: string[];
  /** Messages sent mid-turn that the agent hasn't read yet, oldest first. */
  queued: QueuedMessage[];
}

export interface QueuedMessage {
  text: string;
  files: string[];
  /** Read at the agent's next step rather than after its turn. */
  steer: boolean;
}

export interface BayDto {
  id: BayId;
  kind: 'agent' | 'shell';
  title: string;
  cwd: string;
  folder: string;
  agent: AgentDto | null;
  shell: string | null;
}

export interface FormChoice {
  value: any;
  label: string;
}

export type FormKind =
  | { type: 'text'; multiline: boolean }
  | { type: 'number'; integer: boolean; min: number | null; max: number | null }
  | { type: 'bool' }
  | { type: 'choice'; options: FormChoice[] }
  | { type: 'multi'; options: FormChoice[] };

export interface FormField {
  key: string;
  title: string;
  description: string | null;
  required: boolean;
  kind: FormKind;
  default: any;
}

export type ToolFamily = 'read' | 'edit' | 'create' | 'shell' | 'search' | 'web' | 'task' | 'canvas' | 'memory' | 'skill' | 'sql' | 'graysense' | 'other';

export interface ToolDto {
  type: 'tool';
  name: string;
  label: string;
  verb: string;
  target: string | null;
  family: ToolFamily;
  state: 'running' | 'ok' | 'failed';
  progress: string | null;
  output: string;
  result: string | null;
  error: string | null;
  files: string[];
  elapsed: number | null;
  started: number;
  mcpServer: string | null;
  hasApp: boolean;
  args: any;
}

export interface PermissionDto {
  type: 'permission';
  kind: string;
  what: string;
  summary: string;
  headline: string | null;
  intention: string | null;
  command: string | null;
  warning: string | null;
  fileName: string | null;
  diff: string | null;
  paths: string[];
  url: string | null;
  server: string | null;
  tool: string | null;
  toolDescription: string | null;
  args: string | null;
  canSession: boolean;
  pending: boolean;
  outcome: string | null;
}

export interface UserItem {
  type: 'user';
  text: string;
  files: string[];
  /** How long the agent worked on it (ms), once a turn of its has ended. */
  took?: number | null;
}
export interface AssistantItem {
  type: 'assistant';
  text: string;
  done: boolean;
}
export interface ReasoningItem {
  type: 'reasoning';
  text: string;
  done: boolean;
}
export interface ElicitItem {
  type: 'elicit';
  message: string;
  url: string | null;
  source: string | null;
  fields: FormField[];
  pending: boolean;
  outcome: string | null;
}
export interface AskItem {
  type: 'ask';
  question: string;
  choices: string[];
  freeform: boolean;
  pending: boolean;
  outcome: string | null;
}
export interface PlanItem {
  type: 'plan';
  summary: string;
  plan: string | null;
  actions: string[];
  recommended: string;
  pending: boolean;
  outcome: string | null;
}
export interface SwitchItem {
  type: 'switch';
  text: string;
  pending: boolean;
  outcome: string | null;
}
export interface NoticeItem {
  type: 'notice';
  level: Level;
  text: string;
}
export interface SubagentItem {
  type: 'subagent';
  name: string;
  description: string;
  done: boolean | null;
  tools: number;
  current: string | null;
}
export interface MemoryItem {
  type: 'memory';
  ids: number[];
  text: string;
  learned: boolean;
  undone: boolean;
}
export interface RecallItem {
  type: 'recall';
  entries: { id: number; text: string }[];
}

export type ItemDto =
  | UserItem
  | AssistantItem
  | ReasoningItem
  | ToolDto
  | PermissionDto
  | ElicitItem
  | AskItem
  | PlanItem
  | SwitchItem
  | NoticeItem
  | SubagentItem
  | MemoryItem
  | RecallItem;

export interface TranscriptPatch {
  bay: BayId;
  len: number;
  items: [number, ItemDto][];
}

export type MissionStatus = 'idea' | 'active' | 'review' | 'done';

export interface MissionDto {
  id: number;
  title: string;
  notes: string;
  dir: string | null;
  status: MissionStatus;
  created: number;
  sessionId: string | null;
  fromAgent: boolean;
  bay: BayId | null;
}

export interface PastSessionDto {
  id: string;
  title: string;
  modified: string;
  modifiedTs: number;
  pinned: boolean;
  external: boolean;
  folder: string | null;
  openBay: BayId | null;
}

export interface SessionsDto {
  loaded: boolean;
  items: PastSessionDto[];
}

export type MemoryKind = 'preference' | 'fact' | 'procedure' | 'episode' | 'rule' | 'note';

export interface NoteAttachmentDto {
  kind: 'file' | 'folder' | 'link';
  name: string;
  /** The original path, or the URL. */
  target: string;
  /** Graysense's own copy of a file. */
  stored: string | null;
  size: number | null;
  /** The file (or Graysense's copy) is still there. */
  exists: boolean;
}

export interface MemoryDto {
  id: number;
  kind: MemoryKind;
  kindLabel: string;
  project: string | null;
  projectName: string | null;
  text: string;
  state: 'active' | 'pending' | 'archived';
  origin: 'user' | 'learned' | 'agent';
  confidence: number;
  pinned: boolean;
  /** A change learning proposes: the new text, or "" to delete the memory. */
  proposal: string | null;
  created: number;
  updated: number;
  lastUsed: number | null;
  uses: number;
  reinforced: number;
  sources: { session: string | null; title: string | null; quote: string | null; at: number }[];
  history: { text: string; at: number; why: string }[];
  commands: string[];
  stale: boolean;
  userOwned: boolean;
  archivedReason: string | null;
  /** Notes: files, folders and links kept with them. */
  attachments: NoteAttachmentDto[];
  votes: number;
  opens: number;
  lastOpened: number | null;
  /** Notes: 0..1, how useful lately (votes, opens and recalls, fading over time). */
  relevance: number;
}

export interface MemoryStateDto {
  memories: MemoryDto[];
  lastRun: [number, string | null] | null;
  learning: number;
}

/** A Rust `Result` as serde writes it. */
export type Result<T> = { Ok: T } | { Err: string };

export interface McpServerDto {
  name: string;
  enabled: boolean;
  source: string;
  plugin: string | null;
  config: any;
  status: string | null;
  error: string | null;
}

export interface McpToolDto {
  name: string;
  description: string | null;
  hasUi: boolean;
}

export interface McpDto {
  loading: boolean;
  servers: Result<McpServerDto[]> | null;
  liveBay: BayId | null;
  tools: [string, Result<McpToolDto[]>][];
}

export interface PluginDto {
  name: string;
  marketplace: string;
  version: string | null;
  description: string | null;
  enabled: boolean;
}

export interface MarketDto {
  name: string;
  source: string;
  isDefault: boolean;
}

export interface MarketPluginDto {
  name: string;
  description: string | null;
}

export interface PluginsDto {
  installed: Result<PluginDto[]> | null;
  markets: Result<MarketDto[]> | null;
  browse: [string, Result<MarketPluginDto[]>][];
}

export type ModelCategory = 'powerful' | 'versatile' | 'lightweight';

export interface ModelDto {
  id: string;
  name: string;
  multiplier: number | null;
  category: ModelCategory | null;
  /** Thinking effort levels the model offers, least to most (empty: no setting). */
  efforts: string[];
  defaultEffort: string | null;
}

export interface AppDto {
  /** The model sessions start with when none is chosen, once known. */
  runtimeModel: string | null;
  client: 'idle' | 'connecting' | 'ready' | 'failed';
  clientError: string | null;
  models: ModelDto[];
  focused: BayId | null;
  maximized: boolean;
  version: string;
  sessionsRoot: string;
  defaultDir: string;
  home: string;
  configDir: string;
  search: { building: boolean; built: boolean; sessions: number; files: number };
}

export interface UpdateStatus {
  version: string;
  phase: 'unsupported' | 'idle' | 'checking' | 'upToDate' | 'available' | 'downloading' | 'ready' | 'applying' | 'failed';
  progress: number | null;
  error: string | null;
  nextVersion: string | null;
  nextNotes: string | null;
  checked: number | null;
  via: string | null;
  notesUrl: string;
}

export interface StatsDto {
  cpu: number;
  memUsed: number;
  memTotal: number;
  appMem: number;
}

export interface LogLineDto {
  bay: BayId;
  level: Level;
  text: string;
  at: number;
}

export interface ToastDto {
  id: number;
  level: Level;
  text: string;
  action: [string, any] | null;
}

export type ThemeId = 'graphite' | 'carbon' | 'slate' | 'paper';
export type PanelId = 'missions' | 'sessions' | 'memory' | 'mcp' | 'plugins' | 'settings';
export type LayoutId = 'grid' | 'focus' | 'columns' | 'rows';

/** Settings, under the keys `setSettings` takes (the same keys agents use in `graysense_settings`). */
export interface Settings {
  theme: ThemeId;
  terminal_font_size: number;
  layout: LayoutId;
  side_panel: PanelId | 'hidden';
  side_panel_width: number;
  show_reasoning: boolean;
  cursor_blink: boolean;
  performance_hud: boolean;
  zoom: number;
  default_model: string | null;
  default_effort: string | null;
  default_auto_approve: boolean;
  shell: string | null;
  sessions_folder: string;
  session_folders: boolean;
  auto_update: boolean;
  'memory.enabled': boolean;
  'memory.learn': boolean;
  'memory.share': boolean;
  'memory.review_all': boolean;
  'memory.learning_model': string | null;
  recent_dirs: string[];
}

export interface UiUpdate {
  seq: number;
  full: boolean;
  app?: AppDto;
  settings?: Settings;
  bays?: BayDto[];
  transcripts?: TranscriptPatch[];
  missions?: MissionDto[];
  sessions?: SessionsDto;
  memory?: MemoryStateDto;
  mcp?: McpDto;
  plugins?: PluginsDto;
  update?: UpdateStatus;
  stats?: StatsDto;
  activity?: LogLineDto[];
  toasts?: ToastDto[];
}

export interface SearchHit {
  kind: 'session' | 'file';
  sessionId: string | null;
  path: string | null;
  title: string;
  location: string;
  modified: number;
  snippet: string;
  snippetRanges: [number, number][];
  titleRanges: [number, number][];
  line: number | null;
}

export interface DirEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  modified: number;
}

export interface TerminalShell {
  id: string;
  name: string;
  path: string;
  default: boolean;
}

export interface TerminalInfo {
  id: string;
  shellId: string;
  shellName: string;
  cwd: string;
  pid: number | null;
  windowsBuild: number | null;
}

export type TerminalEvent = { kind: 'output'; data: string } | { kind: 'exit'; code: number | null };

/** An answer to a prompt in a transcript (Rust `Reply`). */
export type Reply =
  | { kind: 'permission'; decision: 'once' | 'session' | 'reject'; feedback?: string | null }
  | { kind: 'ask'; answer: string; freeform: boolean }
  | { kind: 'elicit'; action: 'accept' | 'decline' | 'cancel'; content?: any }
  | { kind: 'plan'; approved: boolean; action?: string | null; feedback?: string | null }
  | { kind: 'switch'; choice: 'yes' | 'always' | 'no' };

/** Everything the core can do (Rust `Action`). */
export type Action =
  | { type: 'newSession'; prompt?: string | null; incognito?: boolean }
  | { type: 'newAgent'; dir: string; prompt?: string | null }
  | { type: 'newShell'; dir?: string | null; shell?: string | null }
  | { type: 'focusBay'; bay: BayId }
  | { type: 'closeBay'; bay: BayId }
  | { type: 'maximizeBay'; bay: BayId }
  | { type: 'moveBay'; bay: BayId; to: number }
  | { type: 'shellTitle'; bay: BayId; title: string }
  /** `queue`: if the agent is mid-turn, wait for the turn to end instead of steering it. */
  | { type: 'send'; bay: BayId; text: string; queue?: boolean }
  | { type: 'abort'; bay: BayId }
  | { type: 'setModel'; bay: BayId; model: string; effort?: string | null }
  | { type: 'setMode'; bay: BayId; mode: string }
  | { type: 'rename'; bay: BayId; name: string }
  | { type: 'attach'; bay: BayId; paths: string[] }
  | { type: 'detach'; bay: BayId; path: string }
  | { type: 'setAutoApprove'; bay: BayId; on: boolean }
  | { type: 'clearTranscript'; bay: BayId }
  | { type: 'reply'; bay: BayId; index: number; reply: Reply }
  | { type: 'forgetMemories'; bay: BayId; index: number }
  | { type: 'closeCanvas'; bay: BayId; instance: string }
  | { type: 'canvasEdit'; bay: BayId; instance: string; action: string; input: any }
  | { type: 'canvasSend'; bay: BayId; instance: string }
  | { type: 'closeApp'; bay: BayId; view: number }
  | { type: 'appCallTool'; view: number; reqId: any; tool: string; args?: any }
  | { type: 'appReadResource'; view: number; reqId: any; uri: string }
  | { type: 'appMessage'; view: number; text: string }
  | { type: 'learnNow'; bay: BayId }
  | { type: 'openPath'; path: string }
  | { type: 'revealPath'; path: string }
  | { type: 'openInVsCode'; path: string }
  | { type: 'resumeSession'; id: string }
  | { type: 'renameSession'; id: string; name: string }
  | { type: 'pinSession'; id: string; pinned: boolean }
  | { type: 'deleteSession'; id: string }
  | { type: 'refreshSessions' }
  | { type: 'missionAdd'; text: string }
  | { type: 'missionEdit'; id: number; title: string; notes: string }
  | { type: 'missionStatus'; id: number; status: MissionStatus }
  | { type: 'missionRemove'; id: number }
  | { type: 'missionUndoRemove' }
  | { type: 'missionSetDir'; id: number; dir: string | null }
  | { type: 'missionLaunch'; id: number }
  | { type: 'missionDelegate'; id: number; bay: BayId }
  | { type: 'memoryAdd'; text: string; kind: MemoryKind; project: string | null }
  | { type: 'memoryEdit'; id: number; text: string; kind: MemoryKind; project: string | null }
  | { type: 'memoryPin'; id: number; pinned: boolean }
  | { type: 'memoryArchive'; id: number }
  | { type: 'memoryRestore'; id: number }
  | { type: 'memoryPurge'; id: number }
  | { type: 'memoryAccept'; id: number }
  | { type: 'memoryDismiss'; id: number }
  | { type: 'memoryAcceptAll' }
  | { type: 'memoryClearAll' }
  | { type: 'memoryExport'; path: string; markdown: boolean }
  | { type: 'memoryImport'; path: string }
  | { type: 'noteAdd'; text: string; files?: string[]; links?: string[]; project?: string | null; session?: string | null; title?: string | null }
  | { type: 'noteAttach'; id: number; files?: string[]; links?: string[] }
  | { type: 'noteDetach'; id: number; index: number }
  | { type: 'noteVote'; id: number; delta: 1 | -1 }
  | { type: 'noteOpen'; id: number; index: number }
  | { type: 'noteReveal'; id: number; index: number }
  | { type: 'noteTouched'; id: number }
  | { type: 'noteAsk'; question: string; ids: number[] }
  | { type: 'setSettings'; changes: Partial<Record<keyof Settings, any>> }
  | { type: 'setSide'; panel: PanelId | 'hidden' | null }
  | { type: 'mcpRefresh' }
  | { type: 'mcpSave'; name: string; config: any; update: boolean }
  | { type: 'mcpRemove'; name: string }
  | { type: 'mcpSetEnabled'; name: string; enabled: boolean }
  | { type: 'mcpTools'; server: string }
  | { type: 'mcpRestart'; server: string }
  | { type: 'mcpStop'; server: string }
  | { type: 'mcpStart'; server: string }
  | { type: 'pluginsRefresh' }
  | { type: 'pluginInstall'; source: string }
  | { type: 'pluginUninstall'; name: string }
  | { type: 'pluginSetEnabled'; name: string; enabled: boolean }
  | { type: 'pluginUpdate'; name: string }
  | { type: 'pluginUpdateAll' }
  | { type: 'marketAdd'; source: string }
  | { type: 'marketRemove'; name: string }
  | { type: 'marketBrowse'; market: string }
  | { type: 'updateCheck' }
  | { type: 'updateDownload' }
  | { type: 'updateRestart'; confirmed: boolean }
  | { type: 'searchRefresh' };
