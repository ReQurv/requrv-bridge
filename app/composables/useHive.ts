import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { openUrl } from '@tauri-apps/plugin-opener'

export type ServiceId = 'opencode' | 'codex' | 'claude_code' | 'hermes' | 'claude_desktop'
export type LaunchMode = 'app' | 'terminal'

export interface ServiceStatus {
  opencode: boolean
  codex: boolean
  claude_code: boolean
  hermes: boolean
  opencode_app: boolean
  opencode_cli: boolean
  codex_app: boolean
  codex_cli: boolean
  codex_app_configured: boolean
  claude_code_cli: boolean
  hermes_app: boolean
  hermes_cli: boolean
  claude_desktop: boolean
  claude_desktop_app: boolean
  claude_desktop_configured: boolean
  opencode_configured: boolean
  claude_code_cli_configured: boolean
}

export interface AppRestartResult {
  restart_required: boolean
}

export interface UpdateInfo {
  current_version: string
  latest_version: string | null
  update_available: boolean
  release_url: string | null
}

export interface HiveModel {
  id: string
  model_type: string
}

export type SurfaceKind = 'desktop' | 'cli'

// One launchable surface of a vendor: a concrete app or CLI. `service` is the
// backend service id, `kind` selects the launch mode (desktop -> app,
// cli -> terminal).
export type ServiceSurface
  = {
    service: ServiceId
    kind: 'desktop'
    name: string
    downloadUrl: string
  }
  | {
    service: ServiceId
    kind: 'cli'
    name: string
    installCommand: string
    windowsInstallCommand?: string
    installDocsUrl: string
  }

// A vendor and the surfaces it exposes. Every vendor offers a Desktop and a CLI
// entry so the rows read the same way, whichever agent they belong to.
export interface ServiceGroup {
  id: string
  title: string
  icon: string
  // Optional in-repo component for vendors with no Simple Icons glyph.
  logo?: string
  description: string
  surfaces: ServiceSurface[]
}

export const SERVICE_GROUPS: ServiceGroup[] = [
  {
    id: 'claude',
    title: 'Claude',
    icon: 'i-simple-icons-claude',
    description: 'App desktop e CLI di Anthropic.',
    surfaces: [
      {
        service: 'claude_desktop',
        kind: 'desktop',
        name: 'Claude Desktop',
        downloadUrl: 'https://claude.com/download'
      },
      {
        service: 'claude_code',
        kind: 'cli',
        name: 'Claude Code',
        installCommand: 'curl -fsSL https://claude.ai/install.sh | bash',
        windowsInstallCommand: 'irm https://claude.ai/install.ps1 | iex',
        installDocsUrl: 'https://code.claude.com/docs/en/setup'
      }
    ]
  },
  {
    id: 'opencode',
    title: 'OpenCode',
    icon: 'i-simple-icons-opencode',
    description: 'App e CLI di OpenCode.',
    surfaces: [
      {
        service: 'opencode',
        kind: 'desktop',
        name: 'OpenCode app',
        downloadUrl: 'https://opencode.ai/download'
      },
      {
        service: 'opencode',
        kind: 'cli',
        name: 'OpenCode CLI',
        installCommand: 'curl -fsSL https://opencode.ai/install | bash',
        windowsInstallCommand: 'npm install -g opencode-ai',
        installDocsUrl: 'https://opencode.ai/docs/'
      }
    ]
  },
  {
    id: 'chatgpt',
    title: 'ChatGPT',
    icon: 'i-simple-icons-openai',
    description: 'ChatGPT.app e la CLI Codex di OpenAI.',
    surfaces: [
      {
        service: 'codex',
        kind: 'desktop',
        name: 'ChatGPT app',
        downloadUrl: 'https://openai.com/chatgpt/download/'
      },
      {
        service: 'codex',
        kind: 'cli',
        name: 'Codex CLI',
        installCommand: 'curl -fsSL https://chatgpt.com/codex/install.sh | sh',
        windowsInstallCommand: 'powershell -ExecutionPolicy ByPass -c "irm https://chatgpt.com/codex/install.ps1 | iex"',
        installDocsUrl: 'https://learn.chatgpt.com/docs/codex/cli'
      }
    ]
  },
  {
    id: 'hermes',
    title: 'Hermes',
    icon: 'i-simple-icons-hermes',
    logo: 'HermesLogo',
    description: 'Hermes Agent di Nous Research.',
    surfaces: [
      {
        service: 'hermes',
        kind: 'desktop',
        name: 'Hermes Desktop',
        downloadUrl: 'https://hermes-agent.nousresearch.com/'
      },
      {
        service: 'hermes',
        kind: 'cli',
        name: 'Hermes CLI',
        installCommand: 'curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash',
        windowsInstallCommand: 'iex (irm https://hermes-agent.nousresearch.com/install.ps1)',
        installDocsUrl: 'https://hermes-agent.nousresearch.com/docs/getting-started/installation'
      }
    ]
  }
]

// Bare vendor name, used where no surface is implied (restart dialogs, errors).
const SERVICE_LABELS: Record<ServiceId, string> = {
  opencode: 'OpenCode',
  codex: 'ChatGPT',
  claude_code: 'Claude Code',
  hermes: 'Hermes',
  claude_desktop: 'Claude'
}

const ALL_SURFACES = SERVICE_GROUPS.flatMap(group => group.surfaces)

export function surfaceKey(surface: ServiceSurface): string {
  return `${surface.service}:${surface.kind}`
}

// Concrete surface name for toasts ("OpenCode CLI avviato").
function surfaceLabel(service: ServiceId, mode: LaunchMode): string {
  const kind: SurfaceKind = mode === 'app' ? 'desktop' : 'cli'
  const surface = ALL_SURFACES.find(s => s.service === service && s.kind === kind)
  return surface?.name ?? SERVICE_LABELS[service]
}

// Installed/configured state of one surface, read from the backend status.
export function surfaceState(st: ServiceStatus | null, surface: ServiceSurface): { installed: boolean, configured: boolean } {
  if (!st) return { installed: false, configured: false }
  if (surface.service === 'claude_desktop') {
    return { installed: st.claude_desktop_app, configured: st.claude_desktop_configured }
  }
  if (surface.service === 'claude_code') {
    return { installed: st.claude_code_cli, configured: st.claude_code_cli_configured }
  }
  if (surface.service === 'hermes') {
    return { installed: surface.kind === 'desktop' ? st.hermes_app : st.hermes_cli, configured: false }
  }
  if (surface.service === 'opencode') {
    return {
      installed: surface.kind === 'desktop' ? st.opencode_app : st.opencode_cli,
      configured: st.opencode_configured
    }
  }
  return {
    installed: surface.kind === 'desktop' ? st.codex_app : st.codex_cli,
    // App and CLI read the same ~/.codex/config.toml.
    configured: st.codex_app_configured
  }
}

const isTauri = computed(() => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window)

const key = ref('')
const keySaved = ref(false)
const saving = ref(false)
const models = ref<HiveModel[]>([])
const selectedModel = ref('')

// Only TEXT_GENERATION models make sense as an LLM; fall back to the full
// list if the gateway ever stops reporting the type.
const chatModels = computed(() => {
  const text = models.value.filter(m => m.model_type === 'TEXT_GENERATION')
  return text.length > 0 ? text : models.value
})
const chatModelIds = computed(() => chatModels.value.map(m => m.id))
const status = ref<ServiceStatus | null>(null)
const refreshing = ref(false)
// Surface key (`<service>:<kind>`) of the tile currently launching: two tiles
// of the same vendor (app and CLI) must not share a busy state.
const launching = ref<string | null>(null)
const keyModalOpen = ref(false)
const restartModalOpen = ref(false)
const restartTarget = ref<ServiceId | null>(null)
// The restart re-applies either the Hive profile (configure flow) or the
// restored state (restore flow): Claude persists settings on shutdown.
const restartAction = ref<'configure' | 'restore'>('configure')
const restarting = ref(false)
const restoring = ref(false)
const updateInfo = ref<UpdateInfo | null>(null)
const checkingForUpdate = ref(false)
const updateDismissed = ref(false)
const projectDirectory = ref('')

export function useHive() {
  const toast = useToast()

  async function refreshStatus() {
    if (!isTauri.value || refreshing.value) return
    refreshing.value = true
    try {
      status.value = await invoke<ServiceStatus>('check_services')
    } catch {
      // stato non determinante: mantieni i valori attuali
    } finally {
      refreshing.value = false
    }
  }

  async function checkForUpdates() {
    if (!isTauri.value || checkingForUpdate.value) return
    checkingForUpdate.value = true
    try {
      updateInfo.value = await invoke<UpdateInfo>('check_for_updates')
    } catch (error) {
      // non critico: la verifica fallita non blocca l'uso dell'app
      toast.add({
        title: 'Verifica aggiornamenti non riuscita',
        description: String(error),
        color: 'warning'
      })
    } finally {
      checkingForUpdate.value = false
    }
  }

  async function loadSavedKey() {
    if (!isTauri.value) return
    try {
      const saved = await invoke<string | null>('get_hive_key')
      if (saved) {
        key.value = saved
        keySaved.value = true
        await loadModels()
      }
    } catch {
      // chiave assente o non leggibile: ignora
    }
  }

  async function loadModels(): Promise<boolean> {
    if (!key.value.trim()) return false
    try {
      models.value = await invoke<HiveModel[]>('list_hive_models', {
        key: key.value.trim()
      })
      const first = chatModelIds.value[0]
      if (first && !chatModelIds.value.includes(selectedModel.value)) {
        selectedModel.value = first
      }
      return true
    } catch (error) {
      toast.add({
        title: 'Impossibile caricare i modelli da AI Hive',
        description: String(error),
        color: 'error'
      })
      return false
    }
  }

  async function saveKey(): Promise<boolean> {
    const trimmed = key.value.trim()
    if (!trimmed || saving.value) return false
    saving.value = true
    try {
      // The backend realigns every config it already wrote with the new key.
      await invoke('set_hive_key', { key: trimmed, model: selectedModel.value })
      key.value = trimmed
      keySaved.value = true
      if (!(await loadModels())) {
        return false
      }
      toast.add({
        title: 'Chiave salvata e verificata',
        description: `${models.value.length} modelli disponibili su AI Hive.`,
        color: 'success'
      })
      return true
    } catch (error) {
      toast.add({
        // The key is saved: only updating an already-written config failed,
        // and the description says which one.
        title: 'Chiave salvata, configurazione da aggiornare',
        description: String(error),
        color: 'warning'
      })
      return false
    } finally {
      saving.value = false
    }
  }

  async function clearKey() {
    if (isTauri.value) {
      try {
        await invoke('delete_hive_key')
      } catch {
        // il file potrebbe già non esistere
      }
    }
    key.value = ''
    keySaved.value = false
    models.value = []
    selectedModel.value = ''
  }

  // The selected directory is shared by CLI launchers for this app session.
  async function chooseProjectDirectory(): Promise<string | null> {
    if (!isTauri.value) return null
    try {
      const selected = await open({
        title: 'Scegli cartella di progetto',
        directory: true,
        multiple: false,
        ...(projectDirectory.value ? { defaultPath: projectDirectory.value } : {})
      })
      if (typeof selected === 'string') projectDirectory.value = selected
      return typeof selected === 'string' ? selected : null
    } catch (error) {
      toast.add({
        title: 'Selezione cartella non riuscita',
        description: String(error),
        color: 'error'
      })
      return null
    }
  }
  // The surface is already selected in the card; configure and launch only that target.

  async function launch(surface: ServiceSurface) {
    const { service } = surface
    const mode: LaunchMode = surface.kind === 'desktop' ? 'app' : 'terminal'
    if (!key.value.trim() || !selectedModel.value || launching.value) return
    launching.value = surfaceKey(surface)

    try {
      const workingDirectory = surface.kind === 'cli'
        ? projectDirectory.value || await chooseProjectDirectory()
        : null
      if (surface.kind === 'cli' && !workingDirectory) return
      if (service === 'codex' && mode === 'app') {
        // Il flusso app configura ChatGPT su AI Hive e, se l'app è già aperta,
        // chiede di riavviarla perché il catalogo modelli si legge all'avvio.
        const result = await invoke<AppRestartResult>('configure_chatgpt_app', {
          model: selectedModel.value,
          key: key.value.trim(),
          models: chatModels.value
        })
        toast.add({
          title: 'ChatGPT configurato su AI Hive',
          color: 'success'
        })
        if (result.restart_required) {
          restartTarget.value = 'codex'
          restartModalOpen.value = true
        } else {
          await invoke('open_chatgpt_app')
        }
        await refreshStatus()
        return
      }
      if (service === 'opencode' && mode === 'app') {
        // L'app legge la config globale all'avvio: se è già aperta serve il
        // riavvio, altrimenti la nuova configurazione non viene caricata.
        const result = await invoke<AppRestartResult>('configure_opencode_app', {
          model: selectedModel.value,
          key: key.value.trim()
        })
        toast.add({
          title: 'OpenCode configurato su AI Hive',
          color: 'success'
        })
        if (result.restart_required) {
          restartTarget.value = 'opencode'
          restartAction.value = 'configure'
          restartModalOpen.value = true
        } else {
          await invoke('open_opencode_app')
        }
        await refreshStatus()
        return
      }
      if (service === 'hermes' && mode === 'app') {
        // Anche l'app desktop si configura con sole variabili d'ambiente, ma
        // un'istanza già aperta non può riceverle: serve il riavvio. (La CLI
        // parte come processo nuovo, quindi passa dal ramo generico.)
        const result = await invoke<AppRestartResult>('launch_hermes_app', {
          model: selectedModel.value,
          key: key.value.trim()
        })
        if (result.restart_required) {
          restartTarget.value = 'hermes'
          restartModalOpen.value = true
        } else {
          toast.add({
            title: 'Hermes avviato su AI Hive',
            color: 'success'
          })
        }
        await refreshStatus()
        return
      }
      if (service === 'claude_desktop') {
        // Il profilo 3p viene scritto in configLibrary: un'istanza già aperta
        // lo legge solo al riavvio, quindi si chiede il restart.
        const result = await invoke<AppRestartResult>('configure_claude_desktop', {
          model: selectedModel.value,
          key: key.value.trim()
        })
        toast.add({
          title: 'Claude Desktop configurato su AI Hive',
          color: 'success'
        })
        if (result.restart_required) {
          restartTarget.value = 'claude_desktop'
          restartAction.value = 'configure'
          restartModalOpen.value = true
        } else {
          await invoke('open_claude_desktop_app')
        }
        await refreshStatus()
        return
      }
      await invoke('launch_service', {
        service,
        model: selectedModel.value,
        key: key.value.trim(),
        mode,
        projectDirectory: workingDirectory,
        models: chatModels.value
      })
      toast.add({
        title: `${surfaceLabel(service, mode)} avviato`,

        color: 'success'
      })
      await refreshStatus()
    } catch (error) {
      toast.add({
        title: `Avvio di ${surfaceLabel(service, mode)} non riuscito`,
        description: String(error),
        color: 'error'
      })
    } finally {
      launching.value = null
    }
  }

  async function confirmRestart() {
    const service = restartTarget.value
    restartModalOpen.value = false
    if (!service || (service !== 'codex' && service !== 'hermes' && service !== 'claude_desktop' && service !== 'opencode')) return
    const label = SERVICE_LABELS[service]
    restarting.value = true
    try {
      if (service === 'codex') {
        await invoke('restart_chatgpt_app')
      } else if (service === 'claude_desktop') {
        if (restartAction.value === 'restore') {
          await invoke('restart_claude_desktop_restored')
        } else {
          await invoke('restart_claude_desktop', {
            model: selectedModel.value,
            key: key.value.trim()
          })
        }
      } else if (service === 'opencode') {
        await invoke('restart_opencode_app')
      } else {
        await invoke('restart_hermes_app', {
          model: selectedModel.value,
          key: key.value.trim()
        })
      }
      toast.add({
        title: `${label} riavviato con AI Hive`,
        color: 'success'
      })
    } catch (error) {
      toast.add({
        title: `Riavvio di ${label} non riuscito`,
        description: String(error),
        color: 'error'
      })
    } finally {
      restarting.value = false
      await refreshStatus()
    }
  }

  function cancelRestart() {
    const service = restartTarget.value
    restartModalOpen.value = false
    if (!service) return
    const label = SERVICE_LABELS[service]
    toast.add({
      title: `${label} si aggiornerà al prossimo avvio`,
      color: 'info'
    })
  }

  // Every target that persists something can be restored; Hermes is env-only,
  // so it never shows the button.
  const RESTORE_COMMANDS: Record<ServiceId, string> = {
    codex: 'restore_chatgpt_app',
    claude_desktop: 'restore_claude_desktop',
    opencode: 'restore_opencode',
    claude_code: 'restore_claude_code_cli',
    hermes: ''
  }

  async function restoreApp(service: ServiceId) {
    const command = RESTORE_COMMANDS[service]
    if (!command || !isTauri.value || restoring.value) return
    restoring.value = true
    const label = SERVICE_LABELS[service]
    try {
      // Only the desktop apps read config at startup, so only they need a
      // restart afterwards to drop the restored state.
      const result = await invoke<AppRestartResult | null>(command)
      toast.add({
        title: `${label} ripristinato`,
        color: 'success'
      })
      if (result?.restart_required) {
        restartTarget.value = service
        restartAction.value = 'restore'
        restartModalOpen.value = true
      } else {
        await refreshStatus()
      }
    } catch (error) {
      toast.add({
        title: `Ripristino di ${label} non riuscito`,
        description: String(error),
        color: 'error'
      })
    } finally {
      restoring.value = false
    }
  }

  async function openExternal(url: string) {
    if (isTauri.value) {
      await openUrl(url)
    } else {
      window.open(url, '_blank', 'noopener,noreferrer')
    }
  }

  return {
    isTauri,
    projectDirectory,
    key,
    keySaved,
    saving,
    models,
    selectedModel,
    status,
    refreshing,
    launching,
    keyModalOpen,
    restartModalOpen,
    restartTarget,
    restarting,
    restoring,
    updateInfo,
    checkingForUpdate,
    updateDismissed,
    chatModels,
    chatModelIds,
    refreshStatus,
    checkForUpdates,
    loadSavedKey,
    saveKey,
    clearKey,
    launch,
    confirmRestart,
    cancelRestart,
    restoreApp,
    openExternal,
    chooseProjectDirectory
  }
}
