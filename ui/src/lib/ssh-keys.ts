import { api } from "./api";
import { useStore } from "./store";

export function promptSshKey(email: string, name: string, onCreated: (path: string, agentAdded: boolean) => void) {
  const { setDialog, toast } = useStore.getState();
  setDialog({
    kind: "form", title: "Generate SSH key", confirm: "Generate",
    fields: [
      { key: "name", label: "Key filename", value: name },
      { key: "passphrase", label: "Passphrase (optional)", type: "password", value: "" },
      { key: "agent", label: "Add to the local SSH agent", type: "checkbox", value: true },
    ],
    onSubmit: async values => {
      try {
        const key = await api.generateSshKey(email, String(values.name), String(values.passphrase), !!values.agent);
        onCreated(key.private_path, key.agent_added);
        const copied = await navigator.clipboard.writeText(key.public_key).then(() => true, () => false);
        if (key.agent_error) toast(false, key.agent_error);
        else toast(true, copied ? "SSH key created and its public key copied" : "SSH key created");
      } catch (error) {
        toast(false, String(error));
      }
    },
  });
}
