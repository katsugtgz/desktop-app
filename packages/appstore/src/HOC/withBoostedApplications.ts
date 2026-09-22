import { graphql } from 'react-apollo';
// import { applicationsLimit, boostedTypes } from '../shared/constants/constants';
// import { QUERY_GET_BOOSTED_APPLICATIONS } from '../graphql/schemes/boostedApplications';

import { ApplicationsAvailable } from '../graphql/queries';

export type BoostedApplicationsRequestVariables = {
  filterByUnifiedSearch: {
    boostedFeatures?: {
      contains: string,
    },
  },
  filterByNotificationBadge: {
    boostedFeatures?: {
      contains: string,
    },
  },
  filterByStatusSync: {
    boostedFeatures?: {
      contains: string,
    },
  },
  sort: {
    field: string,
    direction: string,
  },
  first: number,
};

export type BoostedApplicationsResponse = {
  appsUnifiedSearch?: {list: ApplicationsAvailable[]}
  appNotificationBadge?: {list: ApplicationsAvailable[]},
  appStatusSync?: {list: ApplicationsAvailable[]},
  loading?: boolean,
};

export type WithBoostedApplicationsProps = {
  appsUnifiedSearch?: ApplicationsAvailable[],
  appNotificationBadge?: ApplicationsAvailable[],
  appStatusSync?: ApplicationsAvailable[],
  loading?: boolean,
};

// s11b-appstore-write-commands removed `applications/mockedAllAppsTemp.ts`
// and with it the mock-backed `boostedApplications()` helper. The Boosted
// Apps screen had been rendering the three mocked apps through it since the
// GraphQL query was commented out; the real boosted-applications source is
// not ported yet (no read command models it — s14-appstore-ui-wiring wires
// this screen to the appstore-service crate and decides whether the
// boosted query comes back or the screen is dropped).
export const boostedApplications = (): { apps: ApplicationsAvailable[] } => {
  return {
    apps: [],
    loading: false,
  } as { apps: ApplicationsAvailable[] };
};

// export default graphql<{}, BoostedApplicationsResponse, BoostedApplicationsRequestVariables, WithBoostedApplicationsProps>(
//   QUERY_GET_BOOSTED_APPLICATIONS,
//   {
//     options: () => {
//       return {
//         variables: {
//           filterByUnifiedSearch: {
//             boostedFeatures: {
//               contains: boostedTypes.unifiedSearch.value,
//             },
//           },
//           filterByNotificationBadge: {
//             boostedFeatures: {
//               contains: boostedTypes.notificationBadge.value,
//             },
//           },
//           filterByStatusSync: {
//             boostedFeatures: {
//               contains: boostedTypes.statusSync.value,
//             },
//           },
//           sort: {
//             field: 'name',
//             direction: 'ASC',
//           },
//           first: applicationsLimit,
//         },
//       };
//     },
//     props: ({ data }) => ({
//       appsUnifiedSearch: data && data.appsUnifiedSearch ? data.appsUnifiedSearch.list : [],
//       appNotificationBadge: data && data.appNotificationBadge ? data.appNotificationBadge.list : [],
//       appStatusSync: data && data.appStatusSync ? data.appStatusSync.list : [],
//       loading: data && data.loading,
//     }),
//   },
// );
