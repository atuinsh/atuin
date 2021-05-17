/* -*- Mode: IDL; tab-width: 2; indent-tabs-mode: nil; c-basic-offset: 2 -*- */
/*
 * Clipboard API and events
 * W3C Working Draft, 5 June 2019
 * The origin of this IDL file is:
 * https://www.w3.org/TR/2019/WD-clipboard-apis-20190605/
 */

dictionary ClipboardEventInit : EventInit {
  DataTransfer? clipboardData = null;
};

[Constructor(DOMString type, optional ClipboardEventInit eventInitDict), Exposed=Window]
interface ClipboardEvent : Event {
  readonly attribute DataTransfer? clipboardData;
};

partial interface Navigator {
  [SecureContext, SameObject] readonly attribute Clipboard clipboard;
};

typedef sequence<ClipboardItem> ClipboardItems;

[SecureContext, Exposed=Window] interface Clipboard : EventTarget {
  Promise<ClipboardItems> read();
  Promise<DOMString> readText();
  Promise<undefined> write(ClipboardItems data);
  Promise<undefined> writeText(DOMString data);
};

typedef (DOMString or Blob) ClipboardItemDataType;
typedef Promise<ClipboardItemDataType> ClipboardItemData;

callback ClipboardItemDelayedCallback = ClipboardItemData ();

[Constructor(record<DOMString, ClipboardItemData> items,
    optional ClipboardItemOptions options),
 Exposed=Window] interface ClipboardItem {
  static ClipboardItem createDelayed(
      record<DOMString, ClipboardItemDelayedCallback> items,
      optional ClipboardItemOptions options);

  readonly attribute PresentationStyle presentationStyle;
  readonly attribute long long lastModified;
  readonly attribute boolean delayed;

  readonly attribute FrozenArray<DOMString> types;

  Promise<Blob> getType(DOMString type);
};

enum PresentationStyle { "unspecified", "inline", "attachment" };

dictionary ClipboardItemOptions {
  PresentationStyle presentationStyle = "unspecified";
};

dictionary ClipboardPermissionDescriptor : PermissionDescriptor {
  boolean allowWithoutGesture = false;
};
