<script setup lang="ts">
const hive = useHive()
const { key, keySaved, saving, models, selectedModel, keyModalOpen } = hive

const showKey = ref(false)

async function onSave() {
  const ok = await hive.saveKey()
  if (ok) keyModalOpen.value = false
}
</script>

<template>
  <UModal
    :open="keyModalOpen"
    title="Chiave AI Hive"
    :description="
      keySaved
        ? 'Chiave attiva: gli agenti si avviano puntati ad AI Hive.'
        : 'Incolla la chiave API di hive.requrv.ai per configurare e avviare gli agenti.'
    "
    :ui="{ content: 'max-w-lg' }"
    @update:open="keyModalOpen = $event"
  >
    <template #body>
      <div class="flex flex-col gap-4">
        <UBadge
          v-if="keySaved"
          color="success"
          variant="subtle"
          icon="i-lucide-key-round"
          :label="selectedModel ? `Chiave attiva · ${selectedModel}` : 'Chiave attiva'"
        />
        <UBadge
          v-else
          color="warning"
          variant="subtle"
          icon="i-lucide-key-round"
          label="Nessuna chiave salvata"
        />

        <UInput
          v-model="key"
          :type="showKey ? 'text' : 'password'"
          placeholder="Incolla qui la tua chiave Hive"
          autocomplete="off"
          spellcheck="false"
          class="font-mono"
        >
          <template #trailing>
            <UButton
              :icon="showKey ? 'i-lucide-eye-off' : 'i-lucide-eye'"
              variant="ghost"
              size="sm"
              color="neutral"
              :aria-label="showKey ? 'Nascondi chiave' : 'Mostra chiave'"
              @click="showKey = !showKey"
            />
          </template>
        </UInput>

        <p
          v-if="keySaved && models.length === 0"
          class="text-xs text-dimmed"
        >
          Nessuna voce in /v1/models: la selezione modello resterà vuota.
        </p>
      </div>
    </template>

    <template #footer>
      <div class="flex flex-wrap items-center justify-end gap-3">
        <UButton
          v-if="keySaved"
          label="Rimuovi"
          icon="i-lucide-trash-2"
          color="error"
          variant="ghost"
          @click="hive.clearKey()"
        />
        <UButton
          label="Salva chiave"
          icon="i-lucide-save"
          :loading="saving"
          :disabled="!key.trim()"
          @click="onSave"
        />
      </div>
    </template>
  </UModal>
</template>
