// monaco-editor's deep entry points ship no typings; they expose the same API
// as the package root.
declare module "monaco-editor/editor/editor.main" {
  export * from "monaco-editor";
}
