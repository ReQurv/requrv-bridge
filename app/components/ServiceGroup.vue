<script setup lang="ts">
import type { Component } from 'vue'
import HermesLogo from './HermesLogo.vue'

const props = defineProps<{ group: ServiceGroup }>()

// Vendor marks with no Simple Icons glyph, keyed by ServiceGroup.logo. A
// component reference (not a name) because a runtime `:is="'Name'"` string is
// not seen by the auto-import transform and would render an empty element.
const VENDOR_LOGOS: Record<string, Component> = { HermesLogo }

const hive = useHive()
const { keySaved, selectedModel, status, launching, restoring, projectDirectory } = hive

const rows = computed(() =>
  props.group.surfaces.map(surface => ({
    surface,
    key: surfaceKey(surface),
    ...surfaceState(status.value, surface),
    launchLabel: surface.kind === 'cli' && !projectDirectory.value ? 'Scegli cartella' : `Lancia ${surface.name}`,
    launchIcon: surface.kind === 'cli' ? 'i-lucide-square-terminal' : 'i-lucide-play'
  }))
)
const ready = computed(() => keySaved.value && !!selectedModel.value)
const isWindows = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)
const installTarget = ref<Extract<ServiceSurface, { kind: 'cli' }> | null>(null)
const installCommand = computed(() => {
  const target = installTarget.value
  if (!target) return ''
  return isWindows ? target.windowsInstallCommand ?? target.installCommand : target.installCommand
})
const toast = useToast()
async function copyInstallCommand() {
  try {
    await navigator.clipboard.writeText(installCommand.value)
    toast.add({ title: 'Comando copiato', color: 'success' })
  } catch {
    toast.add({
      title: 'Copia non riuscita',
      description: 'Seleziona il comando nel riquadro e copialo manualmente.',
      color: 'error'
    })
  }
}
const restoreTarget = ref<ServiceSurface | null>(null)
const restoreBody = computed(() =>
  restoreTarget.value?.service === 'claude_desktop'
    ? 'Vengono recuperati i profili pre-Hive e Claude Desktop tornerà a usare l\'account Claude. Il modello Hive sparirà dal picker.'
    : 'Vengono recuperati config.toml e auth.json pre-Hive e ChatGPT tornerà a usare l\'account OpenAI.'
)
</script>

<template>
  <section class="service-card flex flex-col rounded-xl border border-default bg-default p-5 sm:p-6">
    <div class="mb-5 flex min-w-0 items-start gap-3">
      <div class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-elevated text-primary">
        <component
          :is="VENDOR_LOGOS[group.logo!]"
          v-if="group.logo"
          class="h-5 w-auto"
        />
        <UIcon
          v-else
          :name="group.icon"
          class="size-5"
        />
      </div>
      <div class="min-w-0 pt-0.5">
        <h2 class="text-base font-semibold leading-tight text-highlighted">
          {{ group.title }}
        </h2>
        <p class="mt-1 text-sm leading-5 text-muted">
          {{ group.description }}
        </p>
      </div>
    </div>

    <div class="flex flex-col gap-2.5">
      <div
        v-for="row in rows"
        :key="row.key"
        class="flex min-w-0 items-center justify-between gap-3 rounded-lg bg-elevated/60 px-3 py-2.5"
      >
        <div class="min-w-0">
          <p class="truncate text-sm font-medium text-highlighted">
            {{ row.surface.name }}
          </p>
          <p class="mt-0.5 text-xs text-muted">
            {{ row.installed ? 'Installato' : 'Non rilevato' }}
          </p>
        </div>

        <div class="flex shrink-0 items-center gap-1.5">
          <UButton
            v-if="row.installed"
            :label="row.launchLabel"
            :icon="row.launchIcon"
            size="sm"
            :loading="launching === row.key"
            :disabled="!ready"
            @click="hive.launch(row.surface)"
          />
          <UButton
            v-else-if="row.surface.kind === 'cli'"
            label="Istruzioni CLI"
            icon="i-lucide-terminal"
            size="sm"
            color="neutral"
            variant="outline"
            :aria-label="`Istruzioni d'installazione per ${row.surface.name}`"
            @click="installTarget = row.surface"
          />
          <UButton
            v-else
            label="Scarica"
            icon="i-lucide-download"
            size="sm"
            color="neutral"
            variant="outline"
            :aria-label="`Scarica ${row.surface.name}`"
            @click="hive.openExternal(row.surface.downloadUrl)"
          />
          <UButton
            v-if="row.configured"
            icon="i-lucide-rotate-ccw"
            color="neutral"
            variant="ghost"
            :loading="restoring"
            :aria-label="`Ripristina ${row.surface.name}`"
            @click="restoreTarget = row.surface"
          />
        </div>
      </div>
    </div>
  </section>

  <UModal
    :open="!!restoreTarget"
    :title="`Ripristina ${restoreTarget?.name ?? ''}`"
    :description="`Tornare alla configurazione originale di ${restoreTarget?.name ?? ''}?`"
    :ui="{ content: 'max-w-md' }"
    @update:open="restoreTarget = $event ? restoreTarget : null"
  >
    <template #body>
      <p class="text-sm text-muted">
        {{ restoreBody }}
      </p>
    </template>

    <template #footer>
      <div class="flex items-center justify-end gap-3">
        <UButton
          label="Annulla"
          color="neutral"
          variant="ghost"
          @click="restoreTarget = null"
        />
        <UButton
          icon="i-lucide-rotate-ccw"
          label="Ripristina"
          :loading="restoring"
          @click="hive.restoreApp(restoreTarget!.service); restoreTarget = null"
        />
      </div>
    </template>
  </UModal>
  <UModal
    :open="!!installTarget"
    :title="`Installa ${installTarget?.name ?? ''}`"
    :ui="{ content: 'max-w-xl' }"
    @update:open="installTarget = $event ? installTarget : null"
  >
    <template #body>
      <div class="flex flex-col gap-4">
        <p class="text-sm leading-6 text-muted">
          Copia il comando e avvialo nel terminale. Al termine, chiudi questo riquadro e premi «Rileva».
        </p>
        <div class="rounded-lg border border-default bg-elevated/60 p-3">
          <code class="block overflow-x-auto whitespace-pre font-mono text-sm leading-6">{{ installCommand }}</code>
          <div class="mt-3 flex justify-end">
            <UButton
              aria-label="Copia comando"
              icon="i-lucide-copy"
              size="sm"
              color="neutral"
              variant="outline"
              @click="copyInstallCommand"
            />
          </div>
        </div>
      </div>
    </template>

    <template #footer>
      <div class="flex items-center justify-end gap-2">
        <UButton
          label="Chiudi"
          color="neutral"
          variant="ghost"
          @click="installTarget = null"
        />
        <UButton
          label="Documentazione ufficiale"
          icon="i-lucide-external-link"
          color="neutral"
          variant="outline"
          @click="hive.openExternal(installTarget!.installDocsUrl)"
        />
      </div>
    </template>
  </UModal>
</template>
