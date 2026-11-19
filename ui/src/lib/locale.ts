import { useStore, type Settings } from "./store";

const spanish: Readonly<Record<string, string>> = {
  "File": "Archivo", "Edit": "Editar", "View": "Ver", "Help": "Ayuda",
  "New Tab": "Nueva pestaña", "Close Tab": "Cerrar pestaña", "Reopen Closed Tab": "Reabrir pestaña cerrada",
  "Clone Repo...": "Clonar repositorio…", "Init Repo...": "Inicializar repositorio…", "Open Repo...": "Abrir repositorio…",
  "Open Repo in New Window…": "Abrir repositorio en otra ventana…", "Open Repo Management": "Gestionar repositorios",
  "Open External Terminal": "Abrir terminal externa", "Open in File Manager": "Abrir en el gestor de archivos",
  "File History / Blame...": "Historial y autoría del archivo…", "Preferences...": "Preferencias…",
  "Sign Out": "Cerrar sesión", "Exit": "Salir", "Undo": "Deshacer", "Redo": "Rehacer",
  "Cut": "Cortar", "Copy": "Copiar", "Paste": "Pegar", "Select All": "Seleccionar todo",
  "Show Left Panel": "Mostrar panel izquierdo", "Show Commit Details Panel": "Mostrar detalles del commit",
  "Show Toolbar Labels": "Mostrar etiquetas de la barra", "Show Terminal Panel": "Mostrar terminal",
  "Increase Zoom": "Acercar", "Decrease Zoom": "Alejar", "Reset Zoom": "Restablecer zoom",
  "Toggle Full Screen": "Alternar pantalla completa", "Activity Log": "Registro de actividad",
  "Reload Window": "Recargar ventana", "Command Palette": "Paleta de comandos",
  "Keyboard Shortcuts": "Atajos de teclado", "Feature Plan": "Plan de desarrollo",
  "Copy diagnostics": "Copiar diagnóstico", "View Activity Logs": "Ver registro de actividad",
  "View Release Notes": "Ver notas de la versión", "Join Discord": "Unirse a Discord", "About GitCraft": "Acerca de GitCraft",
  "Repositories": "Repositorios", "Open": "Abrir", "Clone": "Clonar", "Create": "Crear",
  "Open Repo": "Abrir repositorio", "Clone Repo": "Clonar repositorio", "Init Repo": "Inicializar repositorio",
  "Search repositories": "Buscar repositorios", "Favorites": "Favoritos", "Recent": "Recientes",
  "Repository Management": "Gestionar repositorios", "No recent repositories yet.": "Todavía no hay repositorios recientes.",
  "Preparing the sample project…": "Preparando el proyecto de ejemplo…", "Try the sample project": "Probar el proyecto de ejemplo",
  "Connect More Integrations": "Conectar más integraciones", "Connect Integrations": "Conectar integraciones",
  "Connect a hosting or issue tracking account to see its pull requests and issues in the left panel and Launchpad.":
    "Conecta una cuenta de alojamiento o seguimiento para ver sus solicitudes de cambio e incidencias en el panel izquierdo y en Launchpad.",
  "Workspaces": "Espacios de trabajo", "Manage workspaces": "Gestionar espacios de trabajo", "Create a workspace": "Crear un espacio de trabajo",
  "Resources": "Recursos", "Preparing…": "Preparando…", "Intro Tutorials": "Tutoriales iniciales",
  "Release Notes": "Notas de la versión", "Documentation": "Documentación",
  "Preferences": "Preferencias", "General": "General", "UI Customization": "Personalización de la interfaz",
  "Theme": "Tema", "Follow system": "Seguir el sistema", "Language": "Idioma", "Date/Time": "Fecha y hora",
  "Pull": "Traer", "Push": "Enviar", "Fetch": "Descargar", "Branch": "Rama", "Stash": "Guardar", "Pop": "Recuperar",
  "Terminal": "Terminal", "Search": "Buscar", "Launchpad": "Launchpad", "Cancel": "Cancelar",
  "Open a Repository": "Abrir un repositorio", "Clone a Repository": "Clonar un repositorio",
  "Create a Repository": "Crear un repositorio", "Stash options": "Opciones de almacenamiento temporal",
  "Close": "Cerrar", "Save": "Guardar", "Accounts": "Cuentas", "Integrations": "Integraciones",
  "Branches": "Ramas", "Local": "Locales", "Remote": "Remotas", "Tags": "Etiquetas", "Stashes": "Cambios guardados",
  "PULL REQUESTS": "SOLICITUDES DE CAMBIO", "MERGE REQUESTS": "SOLICITUDES DE FUSIÓN", "ISSUES": "INCIDENCIAS",
  "Working Directory": "Directorio de trabajo", "Stage All": "Preparar todo", "Unstage All": "Retirar todo",
  "Stage": "Preparar", "Unstage": "Retirar", "Discard": "Descartar", "Commit": "Confirmar cambios",
  "Profiles": "Perfiles", "Commit Signing": "Firma de commits", "Notifications": "Notificaciones",
  "Editor": "Editor", "In-App Terminal": "Terminal integrada", "External Tools": "Herramientas externas",
  "AI": "IA", "Agents": "Agentes", "Experimental": "Experimental", "Git Hooks": "Scripts de Git",
  "Encoding": "Codificación", "Conflict Prevention": "Prevención de conflictos", "Issue Tracker": "Seguimiento de incidencias",
  "Sparse Checkout": "Copia de trabajo parcial", "Pull and Checkout": "Actualización y cambio de rama", "Submodules": "Submódulos",
  "Exit Preferences": "Salir de preferencias", "Current profile": "Perfil actual", "Default Profile": "Perfil predeterminado",
  "Repo-Specific Preferences": "Preferencias del repositorio", "Loading…": "Cargando…",
  "Filter pull requests": "Filtrar solicitudes de cambio", "Closed": "Cerradas", "Merged": "Fusionadas",
  "Created by me": "Creadas por mí", "Review requested": "Revisión solicitada",
  "No matching pull requests.": "No hay solicitudes de cambio que coincidan.",
  "Close dialog": "Cerrar diálogo", "Open a repository": "Abrir un repositorio", "Initialize a Repository": "Inicializar un repositorio",
  "Compare commit ranges": "Comparar rangos de commits", "Recover commits from reflog": "Recuperar commits del reflog",
  "GitCraft feature plan": "Plan de desarrollo de GitCraft", "Clean Up Local Branches": "Limpiar ramas locales",
  "Search for a file to view its history / blame": "Buscar un archivo para ver su historial y autoría",
  "Copy error message to clipboard": "Copiar el mensaje de error", "Add Profile": "Añadir perfil", "Add Remote": "Añadir remoto",
  "Repository folder": "Carpeta del repositorio", "Where to clone to": "Destino de la copia", "Folder name": "Nombre de la carpeta",
  "Initialize submodules": "Inicializar submódulos", "Shallow Clone": "Clonación superficial", "Limit history by": "Limitar historial por",
  "Commit depth": "Profundidad de commits", "Since date": "Desde la fecha", "Depth": "Profundidad",
  "Description": "Descripción", "Repository Name": "Nombre del repositorio", "Remote Name": "Nombre del remoto",
  "Access": "Acceso", "Destination": "Destino", "Project": "Proyecto", "Workspace": "Espacio de trabajo",
  "Name": "Nombre", "Account": "Cuenta", "Pull URL": "URL de lectura", "Push URL (optional)": "URL de escritura (opcional)",
  "Repository URL": "URL del repositorio", "Author": "Autoría", "Default Branch Name": "Nombre de la rama predeterminada",
  "Auto-Fetch Interval": "Intervalo de descarga automática", "Auto-Prune": "Poda automática",
  "Keep submodules up to date": "Mantener actualizados los submódulos",
  "Notification Location": "Posición de las notificaciones", "Top Left": "Arriba a la izquierda",
  "Top Right": "Arriba a la derecha", "Bottom Left": "Abajo a la izquierda", "Bottom Right": "Abajo a la derecha",
  "Date Format": "Formato de fecha", "Time Format": "Formato de hora", "Dark": "Oscuro", "Light": "Claro",
  "Dark (High Contrast)": "Oscuro (alto contraste)", "Light (High Contrast)": "Claro (alto contraste)",
};

export function translate(language: Settings["language"], message: string): string {
  if (language !== "es") return message;
  const prefix = /^(?:✓ |   )/.exec(message)?.[0] ?? "";
  const plain = message.slice(prefix.length);
  return prefix + (Object.hasOwn(spanish, plain) ? spanish[plain] : plain);
}

export function useText(): (message: string) => string {
  const language = useStore(state => state.settings.language);
  return message => translate(language, message);
}
