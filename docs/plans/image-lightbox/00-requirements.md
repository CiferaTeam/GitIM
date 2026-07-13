# Image Lightbox Requirements

## Outcome

Clicking an inline image attachment opens a modal preview inside GitIM. The
conversation remains mounted and the browser does not navigate to the resolver
URL.

## Interaction Contract

- The inline attachment remains a bounded thumbnail with its filename and size.
- Clicking the thumbnail opens a viewport-bounded image preview over the current
  conversation.
- The preview exposes the filename, file size, a Download action, and a visible
  close action.
- Escape, the close action, and clicking outside the preview close it.
- Focus is trapped while the preview is open and returns to the thumbnail after
  close.
- The preview uses the existing verified resolver URL. Download uses the existing
  resolver URL with `download=1`.
- Image load failure continues to use the existing Unavailable and Retry state.
- File cards and Browser-mode Runtime-required metadata keep their existing
  behavior.

## Quality Bar

- The interaction works with pointer and keyboard input.
- The dialog has an accessible name and description.
- Desktop and mobile previews remain inside the visible viewport without page
  overflow.
- Existing message click and double-click boundaries remain isolated.
