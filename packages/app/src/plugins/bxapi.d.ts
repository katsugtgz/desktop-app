import { IpcRendererEvent } from 'electron';
import { ApplicationConfigData } from '../applications/duck';
import { MinimalApplication } from '../applications/graphql/withApplications';
import { BxAppManifest } from '../applications/manifest-provider/bxAppManifest';
import { PopularApps } from '../../manifests';
import { AuthProviders } from '../user-identities/types';

type AppManifest = Omit<BxAppManifest, 'icons'> & { id: string, icon: string };

/**
 * Dead surface removed in w03 (bridge doc §Not ported): `user`, `services`,
 * `Runtime`, `AuthorizationError`, `NoMethodError`, `SystemError` — declared
 * here historically but never constructed by `webview-preload.js`; zero
 * runtime references verified via rg before deletion.
 */

export type PrivateApplicationRequest = {
  name: string,
  themeColor: string,
  bxIconURL: string,
  startURL: string,
  scope: string,
};

declare module BxAPI {

  /**
   * Theme
   * @since 1.11.0
   */
  interface Theme {
    addThemeColorsChangeListener(listener: (event: IpcRendererEvent, themeColors: string[]) => void): void;
  }

  /**
   * NotificationCenter
   * @since 1.12.0
   */
  interface NotificationCenter {
    addSnoozeDurationInMsChangeListener(listener: (event: IpcRendererEvent, duration: string | undefined) => void): void;

    sendNotification(id: string, notification: any);
    closeNotification(id: string);

    addNotificationClickListener(listener: (event: IpcRendererEvent, notificationId: string) => void): void;
    removeNotificationClickListener(listener: (event: IpcRendererEvent, notificationId: string) => void): void;
  }

  interface Manifest {
    getManifest(manifestURL: string): Promise<{ body: BxAppManifest }>,
  }

  interface Applications {
    install(payload: any): Promise<any>,
    uninstall(applicationId: string): Promise<any>,
    uninstallByManifest(manifestURL: string): Promise<any>,
    setConfigData(applicationId: string, configData: ApplicationConfigData): Promise<any>,
    search(query: string): Promise<{ body: MinimalApplication[] }>,
    getMostPopularApps(): Promise<{ body: PopularApps }>,
    getAllCategories(): Promise<{ body: string[]}>,
    getApplicationsByCategory(): Promise<{ body: Record<string, MinimalApplication[]> }>,
    requestPrivate(payload: PrivateApplicationRequest): Promise<{body: { id: string; bxAppManifestURL: string }}>,
    getPrivateApps(): Promise<{ body: AppManifest[] }>,
  }

  interface Identities {
    addIdentitiesChangeListener(listener: (event: IpcRendererEvent, identities: any[]) => void): void;
    removeIdentitiesChangeListener(listener: (event: IpcRendererEvent, identities: any[]) => void): void;
    
    requestLogin(provider: AuthProviders): Promise<any>,
  }
}

/**
 * bx
 * @since 1.11.0
 */
interface Bx {
  theme: BxAPI.Theme,
  notificationCenter: BxAPI.NotificationCenter,
  applications: BxAPI.Applications,
  identities: BxAPI.Identities,
  // Only available on station:// tabs
  manifest: BxAPI.Manifest,
}

declare global {
  interface Window { 
    bxApi: Bx;
  }
}
