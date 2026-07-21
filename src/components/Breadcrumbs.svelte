<script lang="ts">
  let {
    currentInternalPath = '/',
    /** When true, crumbs expose data-drop-folder for in-archive pointer move. */
    canDrop = false,
    onNavigate
  } = $props<{
    currentInternalPath?: string;
    canDrop?: boolean;
    onNavigate: (path: string) => void;
  }>();

  let breadcrumbs = $derived.by(() => {
    let list = [{ name: 'Root', path: '/' }];
    if (!currentInternalPath || currentInternalPath === '/') {
      return list;
    }

    let parts = currentInternalPath.split('/').filter(Boolean);
    let accum = '';
    for (let part of parts) {
      if (accum) {
        accum += '/' + part;
      } else {
        accum = part;
      }
      list.push({ name: part, path: accum });
    }
    return list;
  });

  function handleClick(e: MouseEvent, path: string) {
    e.preventDefault();
    onNavigate(path);
  }
</script>

<nav aria-label="breadcrumb" class="breadcrumbs" title={currentInternalPath || '/'}>
  <ol>
    {#each breadcrumbs as crumb, i (crumb.path)}
      <li
        class="breadcrumbs-item"
        class:active={i === breadcrumbs.length - 1}
        data-drop-folder={canDrop ? crumb.path : undefined}
      >
        {#if i > 0}
          <span class="separator" aria-hidden="true">/</span>
        {/if}
        {#if i === breadcrumbs.length - 1}
          <span title={crumb.path}>{crumb.name}</span>
        {:else}
          <button
            type="button"
            class="link-btn"
            title={canDrop ? `Open or drop here: ${crumb.path}` : crumb.path}
            onclick={(e) => handleClick(e, crumb.path)}
          >{crumb.name}</button>
        {/if}
      </li>
    {/each}
  </ol>
</nav>

<style>
  .link-btn {
    background: transparent;
    border: none;
    padding: 0;
    color: var(--pastel-lavender);
    text-decoration: none;
    cursor: pointer;
    font-family: inherit;
    font-size: inherit;
  }
  .link-btn:hover {
    text-decoration: underline;
  }
  :global(.breadcrumbs-item.drop-folder) {
    outline: 1.5px dashed var(--pastel-mint);
    outline-offset: 2px;
    border-radius: 3px;
    background: rgba(155, 233, 223, 0.12);
  }
  :global(.breadcrumbs-item.drop-folder) .link-btn,
  :global(.breadcrumbs-item.drop-folder) > span:not(.separator) {
    color: var(--pastel-mint);
  }
</style>
