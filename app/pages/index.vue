<script setup lang="ts">
const hive = useHive()
const { isTauri, keySaved, selectedModel, keyModalOpen, refreshing, status, projectDirectory } = hive

const needsSetup = computed(() => !keySaved.value || !selectedModel.value)
const setupHint = computed(() =>
  keySaved.value
    ? 'Seleziona un modello per avviare gli agenti.'
    : 'Configura la chiave AI Hive per avviare gli agenti.'
)
const installedSurfaces = computed(() =>
  SERVICE_GROUPS.flatMap(group => group.surfaces)
    .filter(surface => surfaceState(status.value, surface).installed)
)
const availableSurfaces = computed(() => installedSurfaces.value.length)
const installedDesktop = computed(() => installedSurfaces.value.filter(surface => surface.kind === 'desktop').map(surface => surface.name))
const installedCli = computed(() => installedSurfaces.value.filter(surface => surface.kind === 'cli').map(surface => surface.name))

const activeTab = ref('app')
const tabItems = [
  { label: 'Applicativi', icon: 'i-lucide-app-window', value: 'app', slot: 'app' },
  { label: 'MCP', icon: 'i-lucide-plug', value: 'mcp', slot: 'mcp' }
]
</script>

<template>
  <UPageSection
    id="agents"
    :ui="{ container: 'py-7 sm:py-10' }"
  >
    <main class="mx-auto w-full max-w-6xl px-4 sm:px-6">
      <header class="mb-7 flex flex-col gap-4 sm:mb-8 sm:flex-row sm:items-end sm:justify-between">
        <div class="max-w-2xl">
          <p class="mb-2 text-sm font-medium text-primary-600 dark:text-primary-300">
            AI Hive · ReQurv
          </p>
          <h1 class="font-display text-2xl font-bold tracking-tight text-highlighted sm:text-3xl">
            I tuoi agenti, pronti a partire
          </h1>
          <p class="mt-2 text-sm leading-6 text-muted">
            Scegli un agente installato: verrà avviato con il modello selezionato e collegato ad AI Hive.
          </p>
        </div>

        <div class="flex items-center gap-3">
          <UTooltip
            v-if="status"
            :content="{ side: 'bottom' }"
          >
            <UBadge
              color="success"
              variant="subtle"
              icon="i-lucide-check-circle-2"
              :label="`${availableSurfaces} ${availableSurfaces === 1 ? 'app disponibile' : 'app disponibili'}`"
            />
            <template #content>
              <div
                v-if="availableSurfaces === 0"
                class="whitespace-nowrap text-xs text-muted bg-accented"
              >
                Nessuna app rilevata
              </div>
              <UCard
                v-else
                class="flex flex-col gap-2.5 text-xs"
              >
                <div
                  v-if="installedDesktop.length"
                  class="flex flex-col gap-1"
                >
                  <p class="font-semibold text-highlighted">
                    App desktop
                  </p>
                  <p
                    v-for="name in installedDesktop"
                    :key="name"
                    class="flex items-center gap-1.5 text-muted"
                  >
                    <UIcon
                      name="i-lucide-circle-check"
                      class="size-3.5 text-success"
                    />
                    {{ name }}
                  </p>
                </div>
                <div
                  v-if="installedCli.length"
                  class="flex flex-col gap-1"
                >
                  <p class="font-semibold text-highlighted">
                    CLI
                  </p>
                  <p
                    v-for="name in installedCli"
                    :key="name"
                    class="flex items-center gap-1.5 text-muted"
                  >
                    <UIcon
                      name="i-lucide-circle-check"
                      class="size-3.5 text-success"
                    />
                    {{ name }}
                  </p>
                </div>
              </UCard>
            </template>
          </UTooltip>
          <UButton
            icon="i-lucide-refresh-cw"
            label="Rileva"
            color="neutral"
            variant="outline"
            :loading="refreshing"
            @click="hive.refreshStatus()"
          />
        </div>
      </header>

      <UAlert
        v-if="needsSetup"
        color="warning"
        variant="subtle"
        icon="i-lucide-key-round"
        orientation="horizontal"
        class="mb-6"
        :title="setupHint"
        :actions="[
          {
            label: 'Configura chiave',
            size: 'xs',
            color: 'warning',
            variant: 'solid',
            onClick: () => { keyModalOpen = true }
          }
        ]"
      />
      <UTabs
        v-model="activeTab"
        :items="tabItems"
        class="w-full"
      >
        <template #app>
          <section
            v-if="isTauri"
            class="mb-6 flex flex-col gap-3 rounded-xl border border-default bg-elevated/30 p-4 sm:flex-row sm:items-center sm:justify-between"
          >
            <div class="flex min-w-0 items-start gap-3">
              <div class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-elevated text-primary">
                <UIcon
                  name="i-lucide-folder-open"
                  class="size-5"
                />
              </div>
              <div class="min-w-0">
                <h2 class="text-sm font-semibold text-highlighted">
                  Cartella di progetto
                </h2>
                <p
                  v-if="projectDirectory"
                  class="mt-1 truncate text-sm text-muted"
                  :title="projectDirectory"
                >
                  {{ projectDirectory }}
                </p>
                <p
                  v-else
                  class="mt-1 text-sm text-muted"
                >
                  Scegli dove aprire le CLI. La cartella resta attiva per questa sessione.
                </p>
              </div>
            </div>

            <UButton
              :label="projectDirectory ? 'Cambia cartella' : 'Scegli cartella'"
              icon="i-lucide-folder-open"
              color="neutral"
              variant="outline"
              class="shrink-0"
              @click="hive.chooseProjectDirectory()"
            />
          </section>

          <div class="grid gap-4 md:grid-cols-2">
            <ServiceGroup
              v-for="group in SERVICE_GROUPS"
              :key="group.id"
              :group="group"
            />
          </div>
        </template>
        <template #mcp>
          <McpPanel />
        </template>
      </UTabs>
    </main>
  </UPageSection>
</template>
