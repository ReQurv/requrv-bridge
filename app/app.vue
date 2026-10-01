<script setup>
import { getVersion } from '@tauri-apps/api/app'

const hive = useHive()
const { keySaved, keyModalOpen, selectedModel, chatModelIds, updateInfo, updateDismissed } = hive

const appVersion = ref('')

useHead({
  meta: [{ name: 'viewport', content: 'width=device-width, initial-scale=1' }],
  link: [{ rel: 'icon', href: '/favicon.ico' }],
  htmlAttrs: {
    lang: 'it'
  }
})

const title = 'ReQurv Launch'
const description = 'Lancia i tuoi agenti AI già configurati per AI Hive, l\'AI Gateway di ReQurv.'

useSeoMeta({
  title,
  description,
  ogTitle: title,
  ogDescription: description,
  twitterCard: 'summary_large_image'
})

onMounted(async () => {
  if (hive.isTauri.value) {
    try {
      appVersion.value = await getVersion()
    } catch {
      // versione non leggibile: ignora
    }
  }
  await hive.loadSavedKey()
  hive.refreshStatus()
  hive.checkForUpdates()
  if (!keySaved.value) {
    keyModalOpen.value = true
  }
})
</script>

<template>
  <UApp>
    <div class="flex min-h-svh flex-col">
      <header class="border-b border-default bg-default">
        <div class="mx-auto flex w-full max-w-6xl flex-col gap-3 px-4 py-3 sm:flex-row sm:items-center sm:justify-between sm:px-6">
          <div class="flex min-w-0 items-center gap-3">
            <NuxtLink
              to="/"
              class="focus-visible:outline-3 outline-primary/25 rounded-md p-1 -ms-1"
            >
              <AppLogo class="w-auto h-8 sm:h-10 shrink-0" />
            </NuxtLink>

            <div class="min-w-0">
              <p class="whitespace-nowrap text-sm font-semibold leading-tight">
                ReQurv Launch
              </p>
              <p class="hidden truncate text-xs leading-tight text-muted md:block">
                Lancia i tuoi agenti AI già configurati per AI Hive, l'AI Gateway di ReQurv.
              </p>
            </div>

            <UButton
              label="AI Hive"
              icon="i-lucide-external-link"
              color="neutral"
              variant="ghost"
              size="sm"
              class="ms-2 hidden md:inline-flex"
              @click="hive.openExternal('https://hive.requrv.ai')"
            />
          </div>

          <div class="flex w-full items-center gap-2 sm:w-auto sm:gap-1.5">
            <USelect
              v-if="chatModelIds.length > 0"
              v-model="selectedModel"
              :items="chatModelIds"
              size="sm"
              aria-label="Modello di default"
              class="min-w-0 flex-1 sm:w-40 sm:flex-none lg:w-56"
            />
            <UButton
              :label="keySaved ? 'Chiave AI Hive' : 'Configura chiave'"
              icon="i-lucide-key-round"
              :trailing-icon="keySaved ? 'i-lucide-check' : undefined"
              :color="keySaved ? 'success' : 'warning'"
              :variant="keySaved ? 'subtle' : 'solid'"
              size="sm"
              :ui="{ label: 'hidden sm:inline' }"
              :aria-label="keySaved ? 'Chiave AI Hive' : 'Configura chiave'"
              @click="keyModalOpen = true"
            />
            <UColorModeButton />
          </div>
        </div>
      </header>

      <UContainer
        v-if="updateInfo?.update_available && !updateDismissed"
        class="mt-2"
      >
        <UAlert
          color="warning"
          variant="subtle"
          icon="i-lucide-arrow-up-circle"
          orientation="horizontal"
          title="Nuova versione disponibile"
          :description="`Scarica e avvia l'ultima release (v${updateInfo.latest_version}) per aggiornare ReQurv Launch.`"
          :actions="[
            {
              label: 'Scarica',
              icon: 'i-lucide-download',
              size: 'xs',
              color: 'warning',
              variant: 'solid',
              onClick: () => updateInfo?.release_url && hive.openExternal(updateInfo.release_url)
            }
          ]"
          close
          @update:open="updateDismissed = true"
        />
      </UContainer>

      <UMain class="flex-1 min-h-0">
        <NuxtPage />
      </UMain>

      <USeparator icon="i-lucide-hexagon" />

      <UFooter
        :ui="{
          container: 'flex items-center justify-between gap-x-3 py-3 lg:py-4',
          left: 'order-1 flex min-w-0 items-center gap-x-1.5',
          right: 'order-3 flex shrink-0 items-center justify-end gap-x-1.5'
        }"
      >
        <template #left>
          <p class="truncate text-xs text-muted sm:text-sm">
            ReQurv Launch © {{ new Date().getFullYear() }} — powered by
            <button
              type="button"
              class="cursor-pointer text-primary hover:underline"
              @click="hive.openExternal('https://hive.requrv.ai')"
            >
              AI Hive
            </button>
          </p>
        </template>

        <template #right>
          <p
            v-if="appVersion"
            class="text-xs text-dimmed"
          >
            v{{ appVersion }}
          </p>
        </template>
      </UFooter>
    </div>

    <HiveKeyModal />
    <RestartModal />
  </UApp>
</template>
