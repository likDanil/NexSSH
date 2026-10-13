<script lang="ts">
  // A password field with an eye button that shows and hides what is typed. With `reveal`, the
  // eye on an empty field first fills in the saved password (from the system keychain).
  import type { HTMLInputAttributes } from 'svelte/elements';
  import { t } from '../i18n.svelte';
  import Icon from './Icon.svelte';

  interface Props extends Omit<HTMLInputAttributes, 'type' | 'value'> {
    value: string;
    /** The saved password, asked for when the eye is pressed on an empty field. */
    reveal?: () => Promise<string | null | undefined>;
  }
  let { value = $bindable(), reveal, disabled, ...rest }: Props = $props();

  let shown = $state(false);
  let input = $state<HTMLInputElement>();

  async function toggle() {
    if (!shown && !value && reveal) {
      try {
        value = (await reveal()) ?? '';
      } catch {
        // The keychain could not be read: show the empty field.
      }
    }
    shown = !shown;
    input?.focus();
  }

  // A disabled field (e.g. "forget the saved password") hides its text again.
  $effect(() => {
    if (disabled) shown = false;
  });
</script>

<span class="password">
  <input bind:this={input} {...rest} {disabled} type={shown ? 'text' : 'password'} bind:value spellcheck="false" />
  <button
    type="button"
    class="icon-btn small eye"
    title={t(shown ? 'password.hide' : 'password.show')}
    aria-label={t(shown ? 'password.hide' : 'password.show')}
    aria-pressed={shown}
    {disabled}
    onclick={toggle}
  >
    <Icon name={shown ? 'eyeOff' : 'eye'} size={15} />
  </button>
</span>

<style>
  .password {
    position: relative;
    display: flex;
    width: 100%;
    min-width: 0;
  }
  .password input {
    flex: 1;
    min-width: 0;
    padding-right: 34px;
  }
  .eye {
    position: absolute;
    top: 50%;
    right: 5px;
    transform: translateY(-50%);
  }
</style>
