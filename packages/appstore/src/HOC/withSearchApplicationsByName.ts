// s11b-appstore-write-commands removed `applications/mockedAllAppsTemp.ts`
// and with it the mock-backed `searchAppByName` (dead code: search results
// come from `props.search`, the sdkv2 `searchApplication` channel, already
// ported to `rust/crates/appstore-service`). The commented-out react-apollo
// HOC below is kept for reference until s14-appstore-ui-wiring deletes the
// GraphQL layer wholesale.
import { graphql } from 'react-apollo';

import { ApplicationsAvailable } from '../graphql/queries';
import {
  QUERY_GET_APPLICATIONS,
  ApplicationsRequestVariables,
  ApplicationsResponse,
} from '../graphql/schemes/applications';

export type SearchAppRequestVariables = {
  searchValue: string,
  first: number,
};

export type WithSearchApplicationsByNameProps = {
  apps?: ApplicationsAvailable[],
  loading?: boolean,
};

// export default graphql<
//   SearchAppRequestVariables,
//   ApplicationsResponse,
//   ApplicationsRequestVariables,
//   WithSearchApplicationsByNameProps>(
//   QUERY_GET_APPLICATIONS,
//   {
//     options: ({ searchValue }) => {
//       return {
//         variables: {
//           filters: {
//             search: {
//               contains: searchValue,
//             },
//           },
//           first: applicationsLimit,
//           sort: {
//             field: 'name',
//             direction: 'ASC',
//           },
//         },
//       };
//     },
//     props: ({ data }) => ({
//       apps: data && data.applications ? data.applications.list : [],
//       loading: data && data.loading,
//     }),
//   });
