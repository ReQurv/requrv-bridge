<script setup lang="ts">
const hive = useHive()
const { restartModalOpen, restarting, restartTarget } = hive

const isHermes = computed(() => restartTarget.value === 'hermes')
// Bare vendor names, matching SERVICE_LABELS in useHive.
const APP_LABELS: Record<string, string> = {
  codex: 'ChatGPT',
  opencode: 'OpenCode',
  hermes: 'Hermes',
  claude_desktop: 'Claude'
}
const appLabel = computed(() => APP_LABELS[restartTarget.value ?? ''] ?? 'ChatGPT')
const description = computed(() =>
  isHermes.value
    ? 'La configurazione viene applicata solo al nuovo processo.'
    : 'Le impostazioni vengono lette all\'avvio dell\'app.'
)
const body = computed(() =>
  isHermes.value
    ? 'Riavviare Hermes per applicare la configurazione AI Hive? Una sessione in corso verrà chiusa.'
    : `Riavviare ${appLabel.value} per applicare i cambiamenti? Una sessione in corso verrà chiusa.`
)
</script>

<template>
  <UModal
    :open="restartModalOpen"
    :title="`${appLabel} è in esecuzione`"
    :description="description"
    :ui="{ content: 'max-w-md' }"
    @update:open="restartModalOpen = $event"
  >
    <template #body>
      <p class="text-sm text-muted">
        {{ body }}
      </p>
    </template>

    <template #footer>
      <div class="flex items-center justify-end gap-3">
        <UButton
          label="Più tardi"
          color="neutral"
          variant="ghost"
          @click="hive.cancelRestart()"
        />
        <UButton
          icon="i-lucide-rotate-cw"
          label="Riavvia e apri"
          :loading="restarting"
          @click="hive.confirmRestart()"
        />
      </div>
    </template>
  </UModal>
</template>
