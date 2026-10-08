<script setup lang="ts">
import * as z from 'zod'

const hive = useHive()
const { isTauri, mcpTargets, mcpConfigured } = hive

interface EnvRow {
  name: string
  value: string
}

// Editable shape of one MCP server. `env` is a list of rows in the form and is
// collapsed into a record before being sent to the backend.
interface FormMcp {
  uid: string
  id: string
  name: string
  command: string
  args: string
  targets: string[]
  env: EnvRow[]
  saved: boolean
}

const formMcps = reactive<FormMcp[]>([])
const saving = ref<string | null>(null)
const removing = ref<string | null>(null)
const newSeq = ref(0)

const envRowSchema = z.object({
  name: z.string().regex(/^[A-Za-z_][A-Za-z0-9_]*$/, 'Nome variabile non valido'),
  value: z.string()
})
const mcpSchema = z.object({
  name: z.string().min(1, 'Inserisci un nome'),
  command: z.string().min(1, 'Inserisci il comando'),
  args: z.string(),
  targets: z.array(z.string()).min(1, 'Scegli almeno un agente'),
  env: z.array(envRowSchema)
})

function slugify(name: string): string {
  return name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
}

function newUid(): string {
  newSeq.value += 1
  return `new-${newSeq.value}`
}

function toEnvRecord(rows: EnvRow[]): Record<string, string> {
  const record: Record<string, string> = {}
  for (const row of rows) {
    if (row.name.trim()) record[row.name.trim()] = row.value
  }
  return record
}

function hydrate() {
  formMcps.splice(
    0,
    formMcps.length,
    ...hive.mcpServers.value.map(s => ({
      uid: s.id,
      id: s.id,
      name: s.name,
      command: s.command,
      args: s.args.join(' '),
      targets: [...s.targets],
      env: Object.entries(s.env).map(([name, value]) => ({ name, value: value == null ? '' : String(value) })),
      saved: true
    }))
  )
}

function addMcp() {
  const uid = newUid()
  formMcps.push({ uid, id: '', name: '', command: '', args: '', targets: [], env: [], saved: false })
}

// Stable key for the mcpServers entry: the persisted id once saved, otherwise a
// slug of the name deduped across the list.
function effectiveIds(): string[] {
  const taken = new Set<string>()
  const ids: string[] = []
  formMcps.forEach((mcp, i) => {
    if (mcp.saved) {
      ids[i] = mcp.id
      taken.add(mcp.id)
      return
    }
    const base = slugify(mcp.name) || `mcp-${i + 1}`
    let id = base
    let n = 1
    while (taken.has(id)) {
      n += 1
      id = `${base}-${n}`
    }
    ids[i] = id
    taken.add(id)
  })
  return ids
}
const mcpIds = computed(() => effectiveIds())

const targetItems = computed(() =>
  mcpTargets.value
    .filter(t => t.installed)
    .map(t => ({ label: t.label, value: t.id }))
)

function configuredTargets(mcp: FormMcp): string[] {
  const map = mcpConfigured.value[mcp.saved ? mcp.id : '']
  if (!map) return []
  return Object.entries(map)
    .filter(([, present]) => present)
    .map(([id]) => mcpTargets.value.find(t => t.id === id)?.label ?? id)
}

async function onSave(i: number) {
  const mcp = formMcps[i]
  const id = mcpIds.value[i]
  if (!mcp || !id) return
  const server = {
    id,
    name: mcp.name,
    command: mcp.command,
    args: mcp.args.split(/\s+/).filter(Boolean),
    env: toEnvRecord(mcp.env),
    targets: [...mcp.targets]
  }
  saving.value = mcp.uid
  try {
    const ok = await hive.saveMcpServer(server)
    if (ok) {
      mcp.id = id
      mcp.uid = id
      mcp.saved = true
    }
  } finally {
    saving.value = null
  }
}

async function onRemove(i: number) {
  const mcp = formMcps[i]
  if (!mcp) return
  if (!mcp.saved) {
    formMcps.splice(i, 1)
    return
  }
  removing.value = mcp.uid
  try {
    const ok = await hive.removeMcpServer(mcp.id)
    if (ok) formMcps.splice(i, 1)
  } finally {
    removing.value = null
  }
}

function addEnvRow(mcp: FormMcp) {
  mcp.env.push({ name: '', value: '' })
}

function removeEnvRow(mcp: FormMcp, j: number) {
  mcp.env.splice(j, 1)
}

onMounted(() => {
  void hive.loadMcpServers().then(hydrate)
})
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex flex-col gap-3 rounded-xl border border-default bg-elevated/30 p-4 sm:flex-row sm:items-center sm:justify-between">
      <div>
        <h2 class="text-sm font-semibold text-highlighted">
          Server MCP
        </h2>
        <p class="mt-1 text-sm text-muted">
          Aggiungi server MCP e le variabili d'ambiente. Ogni server viene scritto nella config degli agenti scelti.
        </p>
      </div>
      <UButton
        icon="i-lucide-plus"
        label="Aggiungi server"
        color="neutral"
        variant="outline"
        class="shrink-0"
        :disabled="!isTauri || !targetItems.length"
        @click="addMcp()"
      />
    </div>

    <UAlert
      v-if="isTauri && !targetItems.length"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      orientation="horizontal"
      title="Nessun agente MCP-capace rilevato"
      description="Installa Claude Code, OpenCode o Claude Desktop per configurare i server MCP."
    />

    <div
      v-if="!formMcps.length"
      class="rounded-xl border border-dashed border-default p-8 text-center text-sm text-muted"
    >
      Nessun server MCP. Aggiungi il primo per iniziare.
    </div>

    <UForm
      v-for="(mcp, i) in formMcps"
      :key="mcp.uid"
      :state="mcp"
      :schema="mcpSchema"
      class="flex flex-col gap-4 rounded-xl border border-default bg-elevated/20 p-4 sm:p-5"
      @submit="onSave(i)"
    >
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-center gap-2">
          <UIcon
            name="i-lucide-plug-zap"
            class="size-4 text-primary"
          />
          <span class="font-mono text-xs text-muted">
            {{ mcpIds[i] }}
          </span>
        </div>
        <p
          v-if="configuredTargets(mcp).length"
          class="text-xs text-muted"
        >
          Configurato in: {{ configuredTargets(mcp).join(', ') }}
        </p>
      </div>

      <div class="grid gap-4 sm:grid-cols-2">
        <UFormField
          label="Nome"
          name="name"
        >
          <UInput
            v-model="mcp.name"
            placeholder="Nextcloud"
          />
        </UFormField>
        <UFormField
          label="Comando"
          name="command"
        >
          <UInput
            v-model="mcp.command"
            placeholder="nextcloud-mcp-server"
          />
        </UFormField>
      </div>

      <UFormField
        label="Argomenti"
        name="args"
      >
        <UInput
          v-model="mcp.args"
          placeholder="--transport stdio --port 8080"
          class="w-full"
        />
      </UFormField>

      <UFormField
        label="Agenti"
        name="targets"
      >
        <UCheckboxGroup
          v-model="mcp.targets"
          :items="targetItems"
          class="w-full"
        />
      </UFormField>

      <div class="flex flex-col gap-2">
        <div class="flex items-center justify-between">
          <span class="text-sm font-medium text-highlighted">
            Variabili d'ambiente
          </span>
          <UButton
            color="neutral"
            variant="ghost"
            size="xs"
            icon="i-lucide-plus"
            label="Aggiungi"
            @click="addEnvRow(mcp)"
          />
        </div>
        <UFormField
          :name="`env`"
          :error-pattern="/^env\..+/"
          class="mb-0"
        >
          <div
            v-if="!mcp.env.length"
            class="text-xs text-muted"
          >
            Nessuna variabile d'ambiente.
          </div>
          <div
            v-else
            class="flex flex-col gap-2"
          >
            <div
              v-for="(row, j) in mcp.env"
              :key="j"
              class="grid grid-cols-[1fr_1.4fr_auto] gap-2"
            >
              <UInput
                v-model="row.name"
                placeholder="API_KEY"
                class="font-mono"
              />
              <UInput
                v-model="row.value"
                placeholder="valore"
                class="font-mono"
              />
              <UButton
                color="error"
                variant="ghost"
                size="sm"
                icon="i-lucide-x"
                type="button"
                aria-label="Rimuovi variabile"
                @click="removeEnvRow(mcp, j)"
              />
            </div>
          </div>
        </UFormField>
      </div>

      <div class="flex justify-end gap-2 border-t border-default pt-3">
        <UButton
          color="error"
          variant="ghost"
          size="sm"
          icon="i-lucide-trash-2"
          label="Rimuovi"
          type="button"
          :loading="removing === mcp.uid"
          :disabled="removing !== null && removing !== mcp.uid"
          @click="onRemove(i)"
        />
        <UButton
          type="submit"
          color="primary"
          size="sm"
          icon="i-lucide-save"
          :label="mcp.saved ? 'Aggiorna' : 'Salva'"
          :loading="saving === mcp.uid"
          :disabled="saving !== null && saving !== mcp.uid"
        />
      </div>
    </UForm>
  </div>
</template>
