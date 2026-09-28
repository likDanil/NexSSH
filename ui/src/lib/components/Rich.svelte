<script lang="ts">
  // A translated message whose placeholders are rendered as elements, e.g. a host in
  // <b> or a shortcut in <kbd>, without building HTML strings.
  import { tParts, type MessageKey, type Params } from '../i18n.svelte';

  interface Props {
    key: MessageKey;
    params: Params;
    /** Element per placeholder name; others are plain text. */
    tags?: Record<string, 'b' | 'code' | 'kbd'>;
  }
  let { key, params, tags = {} }: Props = $props();
</script>

{#each tParts(key, params) as part, i (i)}{#if 'param' in part && tags[part.param]}<svelte:element this={tags[part.param]}>{part.value}</svelte:element>{:else if 'param' in part}{part.value}{:else}{part.text}{/if}{/each}
